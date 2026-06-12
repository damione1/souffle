use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use crossbeam_channel::{Receiver, Sender};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::{debug, error, info, warn};

use super::resampler::Resampler;
use crate::state::AudioCommand;

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
                };

                // Block on commands from main thread
                while let Ok(cmd) = cmd_rx.recv() {
                    match cmd {
                        AudioCommand::Start { session_id, target_sample_rate, mic_gain } => {
                            if let Err(e) = capture.start(session_id, target_sample_rate, mic_gain) {
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

    fn start(&mut self, session_id: u64, target_sample_rate: u32, mic_gain: f32) -> Result<(), String> {
        // Ensure any previous callback stops emitting immediately.
        self.active_session_id.store(0, Ordering::Release);
        self.stream.take();

        let device = self.find_device()?;

        let device_name = device.name().unwrap_or_else(|_| "Unknown".into());
        info!("Using input device: {device_name}");

        let config = Self::preferred_config(&device)?;
        let sample_rate = config.sample_rate.0;
        let channels = config.channels;

        info!("Audio config: {sample_rate}Hz, {channels}ch");

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

    fn stop(&mut self) {
        let session_id = self.active_session_id.swap(0, Ordering::AcqRel);
        self.audio_rms.store(0f32.to_bits(), Ordering::Relaxed);

        // Dropping the stream is synchronous — after this, no callback runs.
        let had_stream = self.stream.take().is_some();

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

            // EndOfStream is the signal the actor's stop waits on. The actor
            // is normally draining, so this sends immediately; the timeout
            // only matters if no one is consuming (e.g. session aborted) —
            // then we give up rather than wedge the audio thread, and the
            // actor's own EOS-wait deadline covers the stop path.
            if self
                .audio_sender
                .send_timeout(
                    AudioMessage::EndOfStream { session_id },
                    std::time::Duration::from_secs(1),
                )
                .is_err()
            {
                warn!("Could not deliver end-of-stream marker (channel full or closed)");
            }
        }

        if had_stream {
            info!("Audio capture stopped");
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
