//! System-audio capture via Core Audio process taps (macOS 14.4+).
//!
//! A mono global tap captures the mixed output of all processes *before* it
//! reaches the speakers, so the signal is clean regardless of the user's
//! audio hardware — no virtual driver or manual aggregate device needed.
//! The tap can't be read directly: it must be wrapped in a private,
//! programmatically-created aggregate device whose IO callback delivers the
//! samples. Creating the tap triggers the "System Audio Recording Only"
//! TCC prompt (NSAudioCaptureUsageDescription in Info.plist).
//!
//! The IO callback uses `AudioDeviceCreateIOProcIDWithBlock` with a private
//! dispatch queue. The classic C-IOProc path (`AudioDeviceCreateIOProcID`)
//! must NOT be used here: its HALB_IOThread state machine deadlocks against
//! a cpal/AUHAL input stream in the same process — whichever starts second
//! blocks forever. Verified by the `mic_and_tap_coexist` /
//! `tap_then_mic_coexist` regression tests.
//!
//! Tap CoreAudio calls can also wedge indefinitely when coreaudiod holds
//! state from a crashed client (observed in the wild), so sessions must
//! drive the tap through [`spawn_tap`], which isolates the whole lifecycle
//! on a disposable thread with a startup timeout — a stuck tap degrades the
//! meeting to mic-only instead of freezing the audio thread.
//!
//! All calls must be gated behind `platform::system_audio_capture_supported()`.

#![cfg(target_os = "macos")]

use std::ffi::CStr;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_audio::{
    AudioDeviceCreateIOProcIDWithBlock, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID,
    AudioDeviceStart, AudioDeviceStop, AudioHardwareCreateAggregateDevice,
    AudioHardwareCreateProcessTap, AudioHardwareDestroyAggregateDevice,
    AudioHardwareDestroyProcessTap, AudioObjectGetPropertyData, AudioObjectID,
    AudioObjectPropertyAddress, CATapDescription, kAudioAggregateDeviceIsPrivateKey,
    kAudioAggregateDeviceNameKey, kAudioAggregateDeviceTapAutoStartKey,
    kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey,
    kAudioTapPropertyFormat,
};
use objc2_core_audio_types::{AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};
use ringbuf::HeapProd;
use ringbuf::traits::Producer;
use tracing::{info, warn};

/// State shared with the IO block running on the tap's dispatch queue.
/// The queue is serial, so the producer mutex is never contended.
struct TapShared {
    producer: Mutex<HeapProd<f32>>,
    /// Counts samples dropped because the ring buffer was full.
    dropped: AtomicU64,
    /// Counts IO callback invocations — observable for health checks.
    callbacks: AtomicU64,
}

type IoBlock = RcBlock<
    dyn Fn(
        NonNull<AudioTimeStamp>,
        NonNull<AudioBufferList>,
        NonNull<AudioTimeStamp>,
        NonNull<AudioBufferList>,
        NonNull<AudioTimeStamp>,
    ),
>;

/// Handle to a tap running on its own thread. Dropping it asks that thread
/// to tear the tap down without ever blocking the caller.
pub struct TapHandle {
    /// Closing this channel (on drop) unparks the tap thread.
    _stop_tx: std::sync::mpsc::Sender<()>,
    pub sample_rate: u32,
}

/// Start a system tap on a dedicated thread, waiting up to `timeout` for it
/// to come up. The thread owns the `SystemTap` (which is not `Send`) and
/// drops it when the returned handle is dropped. If CoreAudio wedges —
/// which happens when coreaudiod still holds state from a crashed client —
/// the thread is abandoned and the caller continues without system audio.
pub fn spawn_tap(
    producer: HeapProd<f32>,
    timeout: std::time::Duration,
) -> Result<TapHandle, String> {
    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();

    std::thread::Builder::new()
        .name("system-tap".into())
        .spawn(move || match SystemTap::start(producer) {
            Ok(tap) => {
                if event_tx.send(Ok(tap.sample_rate() as u32)).is_err() {
                    // Caller timed out and gave up; tear down immediately.
                    return;
                }
                // Park until the handle is dropped (recv errors when the
                // sender side closes).
                let _ = stop_rx.recv();
                drop(tap);
            }
            Err(e) => {
                let _ = event_tx.send(Err(e));
            }
        })
        .map_err(|e| format!("Failed to spawn tap thread: {e}"))?;

    match event_rx.recv_timeout(timeout) {
        Ok(Ok(sample_rate)) => Ok(TapHandle {
            _stop_tx: stop_tx,
            sample_rate,
        }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(
            "System tap startup timed out (CoreAudio unresponsive) —              recording microphone only"
                .into(),
        ),
    }
}

/// A running system-audio capture. Samples (mono f32 at `sample_rate()`)
/// are pushed into the ring-buffer producer from the tap's dispatch queue.
/// Dropping tears down the IO proc, aggregate device, and tap.
///
/// Not `Send`: create, use, and drop on the same thread.
pub struct SystemTap {
    tap_id: AudioObjectID,
    aggregate_id: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    shared: Arc<TapShared>,
    /// Keeps the IO block alive for the lifetime of the IO proc (CoreAudio
    /// also retains it, but ownership here makes the lifetime explicit).
    _block: IoBlock,
    /// Retained until AudioDeviceDestroyIOProcID per CoreAudio's contract.
    _queue: DispatchRetained<DispatchQueue>,
    sample_rate: f64,
}

impl SystemTap {
    /// Create a mono global tap and start delivering samples to `producer`.
    pub fn start(producer: HeapProd<f32>) -> Result<Self, String> {
        if !crate::platform::system_audio_capture_supported() {
            return Err("System audio capture requires macOS 14.4 or later".into());
        }

        // Mono mixdown of every process, excluding none.
        let description = unsafe {
            CATapDescription::initMonoGlobalTapButExcludeProcesses(
                CATapDescription::alloc(),
                &NSArray::new(),
            )
        };
        unsafe {
            description.setName(&NSString::from_str("Souffle system audio tap"));
            description.setPrivate(true);
        }

        // This is the call that triggers the TCC permission prompt.
        let mut tap_id: AudioObjectID = 0;
        let status = unsafe { AudioHardwareCreateProcessTap(Some(&description), &mut tap_id) };
        if status != 0 {
            return Err(format!(
                "AudioHardwareCreateProcessTap failed ({status}) — system audio \
                 recording permission may be denied"
            ));
        }

        match Self::build_aggregate(&description, tap_id, producer) {
            Ok(tap) => Ok(tap),
            Err(e) => {
                unsafe { AudioHardwareDestroyProcessTap(tap_id) };
                Err(e)
            }
        }
    }

    fn build_aggregate(
        description: &CATapDescription,
        tap_id: AudioObjectID,
        producer: HeapProd<f32>,
    ) -> Result<Self, String> {
        let sample_rate = tap_stream_format(tap_id)?.mSampleRate;

        let aggregate_dict = aggregate_description(description);

        let mut aggregate_id: AudioObjectID = 0;
        // NSDictionary is toll-free bridged to CFDictionary.
        let cf_dict: &CFDictionary =
            unsafe { &*(Retained::as_ptr(&aggregate_dict) as *const CFDictionary) };
        let status = unsafe {
            AudioHardwareCreateAggregateDevice(cf_dict, NonNull::from(&mut aggregate_id))
        };
        if status != 0 {
            return Err(format!(
                "AudioHardwareCreateAggregateDevice failed ({status})"
            ));
        }

        let shared = Arc::new(TapShared {
            producer: Mutex::new(producer),
            dropped: AtomicU64::new(0),
            callbacks: AtomicU64::new(0),
        });
        let block_shared = Arc::clone(&shared);
        let block: IoBlock = RcBlock::new(
            move |_now: NonNull<AudioTimeStamp>,
                  input_data: NonNull<AudioBufferList>,
                  _input_time: NonNull<AudioTimeStamp>,
                  _output_data: NonNull<AudioBufferList>,
                  _output_time: NonNull<AudioTimeStamp>| {
                forward_input(&block_shared, input_data);
            },
        );

        let queue = DispatchQueue::new("com.souffle.system-tap", None);
        let mut proc_id: AudioDeviceIOProcID = None;
        let status = unsafe {
            AudioDeviceCreateIOProcIDWithBlock(
                NonNull::from(&mut proc_id),
                aggregate_id,
                Some(&queue),
                RcBlock::as_ptr(&block),
            )
        };
        if status != 0 {
            unsafe { AudioHardwareDestroyAggregateDevice(aggregate_id) };
            return Err(format!(
                "AudioDeviceCreateIOProcIDWithBlock failed ({status})"
            ));
        }

        let status = unsafe { AudioDeviceStart(aggregate_id, proc_id) };
        if status != 0 {
            unsafe {
                AudioDeviceDestroyIOProcID(aggregate_id, proc_id);
                AudioHardwareDestroyAggregateDevice(aggregate_id);
            }
            return Err(format!("AudioDeviceStart failed ({status})"));
        }

        info!("System audio tap started ({sample_rate} Hz)");
        Ok(Self {
            tap_id,
            aggregate_id,
            proc_id,
            shared,
            _block: block,
            _queue: queue,
            sample_rate,
        })
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Number of IO callback invocations so far. Zero after a few hundred
    /// ms of running means the tap is not delivering audio (e.g. permission
    /// denied).
    pub fn callback_count(&self) -> u64 {
        self.shared.callbacks.load(Ordering::Relaxed)
    }
}

impl Drop for SystemTap {
    fn drop(&mut self) {
        unsafe {
            AudioDeviceStop(self.aggregate_id, self.proc_id);
            // After this returns no further IO blocks are dispatched; any
            // in-flight one only touches the Arc'd shared state.
            AudioDeviceDestroyIOProcID(self.aggregate_id, self.proc_id);
            AudioHardwareDestroyAggregateDevice(self.aggregate_id);
            AudioHardwareDestroyProcessTap(self.tap_id);
        }
        let dropped = self.shared.dropped.load(Ordering::Relaxed);
        if dropped > 0 {
            warn!("System tap dropped {dropped} samples (ring buffer full)");
        }
        info!("System audio tap stopped");
    }
}

/// Forward the tap's input buffers into the ring buffer. Runs on the tap's
/// serial dispatch queue — the producer mutex is uncontended.
fn forward_input(shared: &TapShared, input_data: NonNull<AudioBufferList>) {
    shared.callbacks.fetch_add(1, Ordering::Relaxed);
    let Ok(mut producer) = shared.producer.lock() else {
        return;
    };

    let list = unsafe { input_data.as_ref() };
    let buffers =
        unsafe { std::slice::from_raw_parts(list.mBuffers.as_ptr(), list.mNumberBuffers as usize) };

    for buffer in buffers {
        if buffer.mData.is_null() {
            continue;
        }
        let samples = unsafe {
            std::slice::from_raw_parts(
                buffer.mData as *const f32,
                buffer.mDataByteSize as usize / size_of::<f32>(),
            )
        };
        let channels = buffer.mNumberChannels.max(1) as usize;
        if channels == 1 {
            let pushed = producer.push_slice(samples);
            shared
                .dropped
                .fetch_add((samples.len() - pushed) as u64, Ordering::Relaxed);
        } else {
            // Defensive: the mono tap shouldn't produce interleaved frames,
            // but downmix without allocating if it ever does.
            for frame in samples.chunks_exact(channels) {
                let mono = frame.iter().sum::<f32>() / channels as f32;
                if producer.try_push(mono).is_err() {
                    shared.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

/// Read the tap's stream format to learn its sample rate.
fn tap_stream_format(tap_id: AudioObjectID) -> Result<AudioStreamBasicDescription, String> {
    let mut address = AudioObjectPropertyAddress {
        mSelector: kAudioTapPropertyFormat,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut format: AudioStreamBasicDescription = unsafe { std::mem::zeroed() };
    let mut size = size_of::<AudioStreamBasicDescription>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            tap_id,
            NonNull::from(&mut address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new(&mut format as *mut _ as *mut std::ffi::c_void)
                .expect("non-null out pointer"),
        )
    };
    if status != 0 {
        return Err(format!("Failed to read tap format ({status})"));
    }
    if format.mSampleRate <= 0.0 {
        return Err("Tap reported an invalid sample rate".into());
    }
    Ok(format)
}

/// The aggregate-device composition wrapping the tap. No output subdevice:
/// the tap drives the aggregate by itself, which keeps it independent of
/// the default output device (no rebuild needed when it changes) and avoids
/// IO contention with other streams of this process.
fn aggregate_description(
    description: &CATapDescription,
) -> Retained<NSDictionary<NSString, AnyObject>> {
    let key = |k: &CStr| NSString::from_str(k.to_str().expect("ASCII key"));
    let aggregate_uid = NSString::from_str(&uuid::Uuid::new_v4().to_string());
    let tap_uid = unsafe { description.UUID().UUIDString() };

    let sub_tap: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
        &[
            &*key(kAudioSubTapUIDKey),
            &*key(kAudioSubTapDriftCompensationKey),
        ],
        &[
            &*tap_uid as &AnyObject,
            &*NSNumber::new_bool(true) as &AnyObject,
        ],
    );

    NSDictionary::from_slices(
        &[
            &*key(kAudioAggregateDeviceNameKey),
            &*key(kAudioAggregateDeviceUIDKey),
            &*key(kAudioAggregateDeviceIsPrivateKey),
            &*key(kAudioAggregateDeviceTapAutoStartKey),
            &*key(kAudioAggregateDeviceTapListKey),
        ],
        &[
            &*NSString::from_str("Souffle Tap") as &AnyObject,
            &*aggregate_uid as &AnyObject,
            &*NSNumber::new_bool(true) as &AnyObject,
            &*NSNumber::new_bool(true) as &AnyObject,
            &*NSArray::from_slice(&[&*sub_tap]) as &AnyObject,
        ],
    )
}

#[cfg(test)]
mod tests {
    use ringbuf::HeapRb;
    use ringbuf::traits::{Observer, Split};

    use super::*;

    /// Needs real audio hardware + the system-audio TCC grant; run manually:
    /// cargo test --lib tap_delivers_samples -- --ignored --nocapture
    #[test]
    #[ignore = "requires audio hardware and TCC permission"]
    fn tap_delivers_samples() {
        let (producer, consumer) = HeapRb::<f32>::new(48_000 * 2).split();
        let tap = SystemTap::start(producer).expect("tap should start");
        assert!(tap.sample_rate() > 0.0);
        eprintln!("tap sample rate: {}", tap.sample_rate());
        std::thread::sleep(std::time::Duration::from_millis(500));
        eprintln!("callbacks: {}", tap.callback_count());
        drop(tap);
        eprintln!("samples: {}", consumer.occupied_len());
        // The tap delivers buffers even when no app is playing (silence).
        assert!(consumer.occupied_len() > 0, "no samples were delivered");
    }

    /// Regression test for the meeting-mode deadlock: a cpal input stream
    /// (AUHAL) and the tap must coexist in the same process. With the old
    /// C-IOProc registration, whichever started second hung forever or got
    /// no IO cycles. Run manually:
    /// cargo test --lib mic_and_tap_coexist -- --ignored --nocapture
    #[test]
    #[ignore = "requires audio hardware and TCC permission"]
    fn mic_and_tap_coexist() {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

        let host = cpal::default_host();
        let device = host.default_input_device().expect("input device");
        let config = device.default_input_config().expect("config");
        let got_data = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let got = got_data.clone();
        let stream = device
            .build_input_stream(
                &config.into(),
                move |_data: &[f32], _: &cpal::InputCallbackInfo| {
                    got.store(true, std::sync::atomic::Ordering::Relaxed);
                },
                |e| eprintln!("stream error: {e}"),
                None,
            )
            .expect("build stream");
        stream.play().expect("play");

        let (producer, consumer) = HeapRb::<f32>::new(48_000 * 2).split();
        let tap = SystemTap::start(producer).expect("tap should start");
        eprintln!("tap rate: {}", tap.sample_rate());

        std::thread::sleep(std::time::Duration::from_millis(700));
        let mic_ok = got_data.load(std::sync::atomic::Ordering::Relaxed);
        let callbacks = tap.callback_count();
        eprintln!(
            "mic fired: {mic_ok}, tap callbacks: {callbacks}, tap samples: {}",
            consumer.occupied_len()
        );
        drop(tap);
        drop(stream);
        assert!(mic_ok, "mic must deliver");
        assert!(callbacks > 0, "tap must deliver alongside the mic");
    }

    /// Same coexistence but tap first — the order start_meeting uses.
    /// cargo test --lib tap_then_mic_coexist -- --ignored --nocapture
    #[test]
    #[ignore = "requires audio hardware and TCC permission"]
    fn tap_then_mic_coexist() {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

        let (producer, consumer) = HeapRb::<f32>::new(48_000 * 2).split();
        let tap = SystemTap::start(producer).expect("tap should start");
        eprintln!("tap rate: {}", tap.sample_rate());

        let host = cpal::default_host();
        let device = host.default_input_device().expect("input device");
        let config = device.default_input_config().expect("config");
        let got_data = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let got = got_data.clone();
        eprintln!("building mic stream...");
        let stream = device
            .build_input_stream(
                &config.into(),
                move |_data: &[f32], _: &cpal::InputCallbackInfo| {
                    got.store(true, std::sync::atomic::Ordering::Relaxed);
                },
                |e| eprintln!("stream error: {e}"),
                None,
            )
            .expect("build stream");
        stream.play().expect("play");
        eprintln!("mic playing");

        std::thread::sleep(std::time::Duration::from_millis(700));
        let mic_ok = got_data.load(std::sync::atomic::Ordering::Relaxed);
        let callbacks = tap.callback_count();
        eprintln!(
            "mic fired: {mic_ok}, tap callbacks: {callbacks}, tap samples: {}",
            consumer.occupied_len()
        );
        drop(stream);
        drop(tap);
        assert!(mic_ok, "mic must deliver");
        assert!(callbacks > 0, "tap must deliver alongside the mic");
    }
}
