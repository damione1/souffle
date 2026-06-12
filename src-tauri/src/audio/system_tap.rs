//! System-audio capture via Core Audio process taps (macOS 14.4+).
//!
//! A mono global tap captures the mixed output of all processes *before* it
//! reaches the speakers, so the signal is clean regardless of the user's
//! audio hardware — no virtual driver or manual aggregate device needed.
//! The tap can't be read directly: it must be wrapped in a private,
//! programmatically-created aggregate device whose IOProc delivers the
//! samples. Creating the tap triggers the "System Audio Recording Only"
//! TCC prompt (NSAudioCaptureUsageDescription in Info.plist).
//!
//! All calls must be gated behind `platform::system_audio_capture_supported()`.

#![cfg(target_os = "macos")]

use std::ffi::{CStr, c_void};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_audio::{
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart,
    AudioDeviceStop, AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap,
    AudioHardwareDestroyAggregateDevice, AudioHardwareDestroyProcessTap,
    AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress, CATapDescription,
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceMainSubDeviceKey,
    kAudioAggregateDeviceNameKey, kAudioAggregateDeviceSubDeviceListKey,
    kAudioAggregateDeviceTapAutoStartKey, kAudioAggregateDeviceTapListKey,
    kAudioAggregateDeviceUIDKey, kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal,
    kAudioSubDeviceUIDKey, kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey,
    kAudioTapPropertyFormat,
};
use objc2_core_audio_types::{AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};
use ringbuf::HeapProd;
use ringbuf::traits::Producer;
use tracing::{info, warn};

use super::output_route;

/// State shared with the HAL IOProc thread. Heap-allocated and owned by
/// `SystemTap`; freed only after the IOProc is destroyed.
struct TapShared {
    producer: HeapProd<f32>,
    /// Counts samples dropped because the ring buffer was full.
    dropped: AtomicU64,
    /// Counts IOProc invocations — observable from other threads for health checks.
    callbacks: AtomicU64,
}

/// A running system-audio capture. Samples (mono f32 at `sample_rate()`)
/// are pushed into the ring-buffer producer from the HAL's IO thread.
/// Dropping tears down the IOProc, aggregate device, and tap.
///
/// Not `Send`: create, use, and drop on the same thread.
pub struct SystemTap {
    tap_id: AudioObjectID,
    aggregate_id: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    shared: *mut TapShared,
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

        let output_device = output_route::default_output_device()?;
        let output_uid = output_route::device_uid(output_device)?;
        let aggregate_dict = aggregate_description(description, &output_uid);

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

        let shared = Box::into_raw(Box::new(TapShared {
            producer,
            dropped: AtomicU64::new(0),
            callbacks: AtomicU64::new(0),
        }));
        let mut proc_id: AudioDeviceIOProcID = None;
        let status = unsafe {
            AudioDeviceCreateIOProcID(
                aggregate_id,
                Some(tap_io_proc),
                shared as *mut c_void,
                NonNull::from(&mut proc_id),
            )
        };
        if status != 0 {
            unsafe {
                drop(Box::from_raw(shared));
                AudioHardwareDestroyAggregateDevice(aggregate_id);
            }
            return Err(format!("AudioDeviceCreateIOProcID failed ({status})"));
        }

        let status = unsafe { AudioDeviceStart(aggregate_id, proc_id) };
        if status != 0 {
            unsafe {
                AudioDeviceDestroyIOProcID(aggregate_id, proc_id);
                drop(Box::from_raw(shared));
                AudioHardwareDestroyAggregateDevice(aggregate_id);
            }
            return Err(format!("AudioDeviceStart failed ({status})"));
        }

        info!("System audio tap started ({sample_rate} Hz, output device {output_uid})");
        Ok(Self {
            tap_id,
            aggregate_id,
            proc_id,
            shared,
            sample_rate,
        })
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Number of IOProc invocations so far. Zero after a few hundred ms of
    /// running means the tap is not delivering audio (e.g. permission denied).
    pub fn callback_count(&self) -> u64 {
        unsafe { (*self.shared).callbacks.load(Ordering::Relaxed) }
    }
}

impl Drop for SystemTap {
    fn drop(&mut self) {
        unsafe {
            AudioDeviceStop(self.aggregate_id, self.proc_id);
            // After this returns the IOProc is guaranteed not to run again,
            // so freeing the shared state is safe.
            AudioDeviceDestroyIOProcID(self.aggregate_id, self.proc_id);
            AudioHardwareDestroyAggregateDevice(self.aggregate_id);
            AudioHardwareDestroyProcessTap(self.tap_id);
            let shared = Box::from_raw(self.shared);
            let dropped = shared.dropped.load(Ordering::Relaxed);
            if dropped > 0 {
                warn!("System tap dropped {dropped} samples (ring buffer full)");
            }
        }
        info!("System audio tap stopped");
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
            NonNull::new(&mut format as *mut _ as *mut c_void).expect("non-null out pointer"),
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

/// The aggregate-device composition wrapping the tap, mirroring what
/// Audio MIDI Setup would build — but private and fully programmatic.
fn aggregate_description(
    description: &CATapDescription,
    output_uid: &str,
) -> Retained<NSDictionary<NSString, AnyObject>> {
    let key = |k: &CStr| NSString::from_str(k.to_str().expect("ASCII key"));
    let aggregate_uid = NSString::from_str(&uuid::Uuid::new_v4().to_string());
    let output_uid = NSString::from_str(output_uid);
    let tap_uid = unsafe { description.UUID().UUIDString() };

    let sub_device: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
        &[&*key(kAudioSubDeviceUIDKey)],
        &[&*output_uid as &AnyObject],
    );
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
            &*key(kAudioAggregateDeviceMainSubDeviceKey),
            &*key(kAudioAggregateDeviceIsPrivateKey),
            &*key(kAudioAggregateDeviceTapAutoStartKey),
            &*key(kAudioAggregateDeviceSubDeviceListKey),
            &*key(kAudioAggregateDeviceTapListKey),
        ],
        &[
            &*NSString::from_str("Souffle Tap") as &AnyObject,
            &*aggregate_uid as &AnyObject,
            &*output_uid as &AnyObject,
            &*NSNumber::new_bool(true) as &AnyObject,
            &*NSNumber::new_bool(true) as &AnyObject,
            &*NSArray::from_slice(&[&*sub_device]) as &AnyObject,
            &*NSArray::from_slice(&[&*sub_tap]) as &AnyObject,
        ],
    )
}

/// HAL IO callback: forward the tap's input buffers into the ring buffer.
/// Runs on a realtime thread — no allocation, no locks, no logging.
unsafe extern "C-unwind" fn tap_io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input_data: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    _output_data: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client_data: *mut c_void,
) -> i32 {
    let shared = unsafe { &mut *(client_data as *mut TapShared) };
    shared.callbacks.fetch_add(1, Ordering::Relaxed);
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
            let pushed = shared.producer.push_slice(samples);
            shared
                .dropped
                .fetch_add((samples.len() - pushed) as u64, Ordering::Relaxed);
        } else {
            // Defensive: the mono tap shouldn't produce interleaved frames,
            // but downmix without allocating if it ever does.
            for frame in samples.chunks_exact(channels) {
                let mono = frame.iter().sum::<f32>() / channels as f32;
                if shared.producer.try_push(mono).is_err() {
                    shared.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
    0
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
}
