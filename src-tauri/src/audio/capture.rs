use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use crossbeam_channel::{Receiver, Sender};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{info, warn};

use super::resampler::Resampler;
use crate::state::AudioCommand;

#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub session_id: u64,
    pub samples: Vec<f32>,
}

/// Info about an available audio input device, sent to frontend
#[derive(Debug, Clone, serde::Serialize)]
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
            eprintln!("[souffle] Failed to list input devices: {e}");
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
    audio_sender: Sender<AudioChunk>,
    selected_device: Option<String>,
    active_session_id: Arc<AtomicU64>,
}

impl AudioCapture {
    /// Spawn the audio thread. Returns channels for commanding it and receiving audio.
    pub fn spawn() -> (Sender<AudioCommand>, Receiver<AudioChunk>) {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<AudioCommand>();
        // Bounded channel: ~30 seconds of audio at 24kHz in 1-second chunks
        let (audio_tx, audio_rx) = crossbeam_channel::bounded::<AudioChunk>(30);

        std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || {
                let mut capture = AudioCapture {
                    stream: None,
                    audio_sender: audio_tx,
                    selected_device: None,
                    active_session_id: Arc::new(AtomicU64::new(0)),
                };

                // Block on commands from main thread
                while let Ok(cmd) = cmd_rx.recv() {
                    match cmd {
                        AudioCommand::Start(session_id) => {
                            if let Err(e) = capture.start(session_id) {
                                eprintln!("[souffle] Failed to start audio capture: {e}");
                            }
                        }
                        AudioCommand::Stop => {
                            capture.stop();
                        }
                        AudioCommand::SelectDevice(name) => {
                            eprintln!("[souffle] Selected input device: {name}");
                            capture.selected_device = Some(name);
                        }
                    }
                }

                info!("Audio thread exiting");
            })
            .expect("Failed to spawn audio thread");

        (cmd_tx, audio_rx)
    }

    fn find_device(&self) -> Result<Device, String> {
        let host = cpal::default_host();

        if let Some(ref name) = self.selected_device {
            // Find the device by name
            let devices = host
                .input_devices()
                .map_err(|e| format!("Failed to list devices: {e}"))?;

            for device in devices {
                if let Ok(n) = device.name() {
                    if n == *name {
                        return Ok(device);
                    }
                }
            }
            eprintln!("[souffle] Selected device '{name}' not found, falling back to default");
        }

        host.default_input_device()
            .ok_or_else(|| "No input device available".to_string())
    }

    fn start(&mut self, session_id: u64) -> Result<(), String> {
        // Ensure any previous callback stops emitting immediately.
        self.active_session_id.store(0, Ordering::Release);
        self.stream.take();

        let device = self.find_device()?;

        let device_name = device.name().unwrap_or_else(|_| "Unknown".into());
        eprintln!("[souffle] Using input device: {device_name}");

        let config = Self::preferred_config(&device)?;
        let sample_rate = config.sample_rate.0;
        let channels = config.channels;

        eprintln!("[souffle] Audio config: {sample_rate}Hz, {channels}ch");

        let mut resampler = Resampler::new(sample_rate, channels);
        let sender = self.audio_sender.clone();
        let active_session_id = Arc::clone(&self.active_session_id);

        let err_fn = |err: cpal::StreamError| {
            eprintln!("[souffle] Audio stream error: {err}");
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

                    let resampled = resampler.process(data);
                    if !resampled.is_empty() {
                        // Log first chunk to confirm audio is flowing
                        if crate::debug::transcription_debug_enabled()
                            && !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed)
                        {
                            let max_amp = resampled.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
                            eprintln!(
                                "[souffle] First audio chunk: {} samples, max_amp={max_amp:.4}",
                                resampled.len(),
                            );
                        }
                        if sender
                            .try_send(AudioChunk {
                                session_id,
                                samples: resampled,
                            })
                            .is_err()
                        {
                            warn!("Audio buffer full, dropping samples");
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

        eprintln!("[souffle] Audio capture started on '{device_name}'");
        Ok(())
    }

    fn stop(&mut self) {
        self.active_session_id.store(0, Ordering::Release);
        if self.stream.take().is_some() {
            eprintln!("[souffle] Audio capture stopped");
        }
    }

    fn preferred_config(device: &Device) -> Result<StreamConfig, String> {
        let supported = device
            .supported_input_configs()
            .map_err(|e| format!("Failed to get supported configs: {e}"))?;

        let config = supported
            .filter(|c| c.sample_format() == SampleFormat::F32)
            .next()
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
