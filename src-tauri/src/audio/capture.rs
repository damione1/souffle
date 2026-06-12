use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use ringbuf::HeapRb;
use ringbuf::traits::{Producer, Split};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

use super::mixer::MeetingMixer;
use super::resampler::Resampler;
use crate::state::AudioCommand;

/// Mixer cadence while a meeting session is active. Rings hold ~2s, so the
/// 5ms tick has plenty of slack; it only exists to pace the mixer, the
/// real-time callbacks never wait on it.
const MEETING_TICK: Duration = Duration::from_millis(5);

/// Per-session state for meeting mode (mic + system audio).
struct MeetingState {
    session_id: u64,
    mixer: MeetingMixer,
    /// None when tap creation failed — the mixer then runs mic-only.
    #[cfg(target_os = "macos")]
    tap: Option<super::system_tap::SystemTap>,
}

#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub session_id: u64,
    pub samples: Vec<f32>,
    /// When the chunk left the capture callback — used for lag tracking.
    pub captured_at: Instant,
}

/// Messages flowing from the capture thread to the engine actor.
#[derive(Debug, Clone)]
pub enum AudioMessage {
    Chunk(AudioChunk),
    /// Sent after the cpal stream is dropped and the resampler flushed —
    /// guaranteed to be the last message of a session, so the actor can
    /// drain deterministically instead of sleeping.
    EndOfStream { session_id: u64 },
}

/// Info about an available audio input device, sent to frontend
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub is_default: bool,
}

/// List all available input devices
pub fn list_input_devices() -> Vec<AudioDeviceInfo> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();

    let devices = match host.input_devices() {
        Ok(d) => d,
        Err(e) => {
            warn!("Failed to list input devices: {e}");
            return vec![];
        }
    };

    devices
        .filter_map(|d| {
            let name = d.name().ok()?;
            Some(AudioDeviceInfo {
                is_default: name == default_name,
                name,
            })
        })
        .collect()
}

/// Manages audio capture from a selected input device.
/// Sends resampled 24kHz mono f32 chunks over a crossbeam channel.
///
/// This struct lives on a dedicated thread because cpal's Stream is !Send on macOS.
pub struct AudioCapture {
    stream: Option<Stream>,
    audio_sender: Sender<AudioMessage>,
    selected_device: Option<String>,
    active_session_id: Arc<AtomicU64>,
    audio_rms: Arc<AtomicU32>,
    /// Shared with the cpal callback so stop() can flush the resampler tail
    /// after the stream is dropped (no callback runs by then, so the lock
    /// is uncontended).
    resampler: Option<Arc<Mutex<Resampler>>>,
    /// Counts chunks dropped because the audio channel was full.
    dropped_counter: Arc<AtomicU64>,
    /// Active meeting-mode session (mic + system audio mixed on this thread).
    meeting: Option<MeetingState>,
}

impl AudioCapture {
    /// Spawn the audio thread. Returns channels for commanding it and receiving audio.
    /// `dropped_counter` is incremented for every chunk lost to a full channel;
    /// the engine actor resets and reads it for health reporting.
    pub fn spawn(
        audio_rms: Arc<AtomicU32>,
        dropped_counter: Arc<AtomicU64>,
    ) -> Result<(Sender<AudioCommand>, Receiver<AudioMessage>), String> {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<AudioCommand>();
        // Bounded channel: many small chunks per second from cpal; inference (Kyutai/Metal)
        // can lag real-time. If this fills, try_send drops audio while RMS/waveform still
        // updates — use a generous bound so the inference thread can catch up.
        let (audio_tx, audio_rx) = crossbeam_channel::bounded::<AudioMessage>(512);

        std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || {
                let mut capture = AudioCapture {
                    stream: None,
                    audio_sender: audio_tx,
                    selected_device: None,
                    active_session_id: Arc::new(AtomicU64::new(0)),
                    audio_rms,
                    resampler: None,
                    dropped_counter,
                    meeting: None,
                };

                // Block on commands; while a meeting is active, wake every
                // few ms to run the mixer instead.
                loop {
                    let cmd = if capture.meeting.is_some() {
                        match cmd_rx.recv_timeout(MEETING_TICK) {
                            Ok(cmd) => cmd,
                            Err(RecvTimeoutError::Timeout) => {
                                capture.meeting_tick();
                                continue;
                            }
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                    } else {
                        match cmd_rx.recv() {
                            Ok(cmd) => cmd,
                            Err(_) => break,
                        }
                    };

                    match cmd {
                        AudioCommand::Start {
                            session_id,
                            target_sample_rate,
                            mic_gain,
                            capture_system_audio,
                        } => {
                            if let Err(e) = capture.start(
                                session_id,
                                target_sample_rate,
                                mic_gain,
                                capture_system_audio,
                            ) {
                                warn!("Failed to start audio capture: {e}");
                            }
                        }
                        AudioCommand::Stop => {
                            capture.stop();
                        }
                        AudioCommand::SelectDevice(name) => {
                            info!("Selected input device: {name}");
                            capture.selected_device = Some(name);
                        }
                    }
                }

                info!("Audio thread exiting");
            })
            .map_err(|e| format!("Failed to spawn audio thread: {e}"))?;

        Ok((cmd_tx, audio_rx))
    }

    fn find_device(&self) -> Result<Device, String> {
        let host = cpal::default_host();

        if let Some(ref name) = self.selected_device {
            // Find the device by name
            let devices = host
                .input_devices()
                .map_err(|e| format!("Failed to list devices: {e}"))?;

            for device in devices {
                if let Ok(n) = device.name()
                    && n == *name
                {
                    return Ok(device);
                }
            }
            warn!("Selected device '{name}' not found, falling back to default");
        }

        host.default_input_device()
            .ok_or_else(|| "No input device available".to_string())
    }

    fn start(
        &mut self,
        session_id: u64,
        target_sample_rate: u32,
        mic_gain: f32,
        capture_system_audio: bool,
    ) -> Result<(), String> {
        // Ensure any previous callback stops emitting immediately, and tear
        // down any leftover meeting state (tap included).
        self.active_session_id.store(0, Ordering::Release);
        self.stream.take();
        self.meeting.take();

        let device = self.find_device()?;

        let device_name = device.name().unwrap_or_else(|_| "Unknown".into());
        info!("Using input device: {device_name}");

        let config = Self::preferred_config(&device)?;
        let sample_rate = config.sample_rate.0;
        let channels = config.channels;

        info!("Audio config: {sample_rate}Hz, {channels}ch");

        if capture_system_audio {
            return self.start_meeting(
                &device,
                &config,
                session_id,
                target_sample_rate,
                mic_gain,
            );
        }

        let resampler = Arc::new(Mutex::new(Resampler::new(
            sample_rate,
            channels,
            target_sample_rate,
            mic_gain,
        )));
        self.resampler = Some(Arc::clone(&resampler));
        let sender = self.audio_sender.clone();
        let active_session_id = Arc::clone(&self.active_session_id);
        let rms_ref = Arc::clone(&self.audio_rms);
        let dropped_counter = Arc::clone(&self.dropped_counter);

        let err_fn = |err: cpal::StreamError| {
            error!("Audio stream error: {err}");
        };

        // Reset the first-chunk logging flag for each new recording session
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        LOGGED.store(false, std::sync::atomic::Ordering::Relaxed);

        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if active_session_id.load(Ordering::Acquire) != session_id {
                        return;
                    }

                    let resampled = match resampler.lock() {
                        Ok(mut r) => r.process(data),
                        Err(_) => return,
                    };
                    if !resampled.is_empty() {
                        // Compute RMS for waveform visualization
                        let sum_sq: f32 = resampled.iter().map(|s| s * s).sum();
                        let rms = (sum_sq / resampled.len() as f32).sqrt();
                        // Clamp to 0.0-1.0 (typical speech RMS is 0.01-0.15)
                        let normalized = (rms * 8.0).min(1.0);
                        rms_ref.store(normalized.to_bits(), Ordering::Relaxed);

                        // Log first chunk to confirm audio is flowing
                        if crate::debug::transcription_debug_enabled()
                            && !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed)
                        {
                            let max_amp = resampled.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
                            debug!(
                                "First audio chunk: {} samples, max_amp={max_amp:.4}",
                                resampled.len(),
                            );
                        }
                        if sender
                            .try_send(AudioMessage::Chunk(AudioChunk {
                                session_id,
                                samples: resampled,
                                captured_at: Instant::now(),
                            }))
                            .is_err()
                        {
                            let dropped = dropped_counter.fetch_add(1, Ordering::Relaxed) + 1;
                            if dropped == 1 || dropped.is_multiple_of(100) {
                                warn!("Audio buffer full, dropping samples ({dropped} chunks dropped this session)");
                            }
                        }
                    }
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to build input stream: {e}"))?;

        self.active_session_id.store(session_id, Ordering::Release);
        stream
            .play()
            .map_err(|e| format!("Failed to start stream: {e}"))?;
        self.stream = Some(stream);

        info!("Audio capture started on '{device_name}'");
        Ok(())
    }

    /// Meeting mode: the cpal callback only pushes raw samples into a ring
    /// buffer; a system-audio tap fills a second ring; `meeting_tick()` on
    /// this thread resamples, mixes, and forwards to the engine.
    fn start_meeting(
        &mut self,
        device: &Device,
        config: &StreamConfig,
        session_id: u64,
        target_sample_rate: u32,
        mic_gain: f32,
    ) -> Result<(), String> {
        let sample_rate = config.sample_rate.0;
        let channels = config.channels;

        // ~2s of headroom per ring; the 5ms tick drains far faster.
        let mic_capacity = (sample_rate as usize * channels as usize) * 2;
        let (mut mic_prod, mic_cons) = HeapRb::<f32>::new(mic_capacity).split();
        let (tap_prod, tap_cons) = HeapRb::<f32>::new(super::mixer::MIX_RATE as usize * 2).split();

        #[cfg(target_os = "macos")]
        let (tap, tap_rate) = match super::system_tap::SystemTap::start(tap_prod) {
            Ok(tap) => {
                let rate = tap.sample_rate() as u32;
                (Some(tap), rate)
            }
            Err(e) => {
                warn!("System audio capture unavailable, recording mic only: {e}");
                (None, super::mixer::MIX_RATE)
            }
        };
        #[cfg(not(target_os = "macos"))]
        let tap_rate = {
            drop(tap_prod);
            super::mixer::MIX_RATE
        };

        let mixer = MeetingMixer::new(
            mic_cons,
            sample_rate,
            channels,
            mic_gain,
            tap_cons,
            tap_rate,
            target_sample_rate,
        );

        let active_session_id = Arc::clone(&self.active_session_id);
        let err_fn = |err: cpal::StreamError| {
            error!("Audio stream error: {err}");
        };
        let stream = device
            .build_input_stream(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if active_session_id.load(Ordering::Acquire) != session_id {
                        return;
                    }
                    // Ring full means the mixer is wedged; losing mic samples
                    // here is the only safe option in a realtime callback.
                    let _ = mic_prod.push_slice(data);
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to build input stream: {e}"))?;

        self.active_session_id.store(session_id, Ordering::Release);
        stream
            .play()
            .map_err(|e| format!("Failed to start stream: {e}"))?;
        self.stream = Some(stream);
        self.meeting = Some(MeetingState {
            session_id,
            mixer,
            #[cfg(target_os = "macos")]
            tap,
        });

        info!("Meeting audio capture started (mic + system audio)");
        Ok(())
    }

    /// Periodic mixer pump while a meeting session is active.
    fn meeting_tick(&mut self) {
        let Some(meeting) = self.meeting.as_mut() else {
            return;
        };
        let samples = meeting.mixer.tick();
        if samples.is_empty() {
            return;
        }

        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        let rms = (sum_sq / samples.len() as f32).sqrt();
        self.audio_rms
            .store((rms * 8.0).min(1.0).to_bits(), Ordering::Relaxed);

        if self
            .audio_sender
            .try_send(AudioMessage::Chunk(AudioChunk {
                session_id: meeting.session_id,
                samples,
                captured_at: Instant::now(),
            }))
            .is_err()
        {
            let dropped = self.dropped_counter.fetch_add(1, Ordering::Relaxed) + 1;
            if dropped == 1 || dropped.is_multiple_of(100) {
                warn!("Audio buffer full, dropping samples ({dropped} chunks dropped this session)");
            }
        }
    }

    fn stop(&mut self) {
        let session_id = self.active_session_id.swap(0, Ordering::AcqRel);
        self.audio_rms.store(0f32.to_bits(), Ordering::Relaxed);

        // Dropping the stream is synchronous — after this, no callback runs.
        let had_stream = self.stream.take().is_some();

        if let Some(mut meeting) = self.meeting.take() {
            // Tear down the tap first so its ring stops filling; then one
            // final flush drains both rings and all resampler tails.
            #[cfg(target_os = "macos")]
            meeting.tap.take();

            if session_id != 0 {
                let tail = meeting.mixer.flush();
                if !tail.is_empty() {
                    let _ = self.audio_sender.send(AudioMessage::Chunk(AudioChunk {
                        session_id,
                        samples: tail,
                        captured_at: Instant::now(),
                    }));
                }
                let discarded = meeting.mixer.tap_discarded();
                if discarded > 0 {
                    warn!("Discarded {discarded} system-audio samples to bound drift");
                }
                self.send_end_of_stream(session_id);
            }

            if had_stream {
                info!("Meeting audio capture stopped");
            }
            return;
        }

        if session_id != 0 {
            // Flush the resampler's remaining partial chunk so the last
            // spoken samples reach the engine instead of being discarded.
            if let Some(resampler) = self.resampler.take()
                && let Ok(mut r) = resampler.lock()
            {
                let tail = r.flush();
                if !tail.is_empty() {
                    let _ = self.audio_sender.send(AudioMessage::Chunk(AudioChunk {
                        session_id,
                        samples: tail,
                        captured_at: Instant::now(),
                    }));
                }
            }

            self.send_end_of_stream(session_id);
        }

        if had_stream {
            info!("Audio capture stopped");
        }
    }

    /// EndOfStream is the signal the actor's stop waits on. The actor is
    /// normally draining, so this sends immediately; the timeout only
    /// matters if no one is consuming (e.g. session aborted) — then we give
    /// up rather than wedge the audio thread, and the actor's own EOS-wait
    /// deadline covers the stop path.
    fn send_end_of_stream(&self, session_id: u64) {
        if self
            .audio_sender
            .send_timeout(
                AudioMessage::EndOfStream { session_id },
                Duration::from_secs(1),
            )
            .is_err()
        {
            warn!("Could not deliver end-of-stream marker (channel full or closed)");
        }
    }

    fn preferred_config(device: &Device) -> Result<StreamConfig, String> {
        let mut supported = device
            .supported_input_configs()
            .map_err(|e| format!("Failed to get supported configs: {e}"))?;

        let config = supported
            .find(|c| c.sample_format() == SampleFormat::F32)
            .or_else(|| {
                device
                    .supported_input_configs()
                    .ok()
                    .and_then(|mut c| c.next())
            })
            .ok_or_else(|| "No supported input config found".to_string())?;

        Ok(config.with_max_sample_rate().into())
    }
}
