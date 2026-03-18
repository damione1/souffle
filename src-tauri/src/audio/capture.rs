use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use crossbeam_channel::{Receiver, Sender};
use tracing::{error, info, warn};

use super::resampler::Resampler;
use crate::state::AudioCommand;

/// Manages audio capture from the system's default input device.
/// Sends resampled 16kHz mono f32 chunks over a crossbeam channel.
///
/// This struct lives on a dedicated thread because cpal's Stream is !Send on macOS.
pub struct AudioCapture {
    stream: Option<Stream>,
    audio_sender: Sender<Vec<f32>>,
}

impl AudioCapture {
    /// Spawn the audio thread. Returns channels for commanding it and receiving audio.
    pub fn spawn() -> (Sender<AudioCommand>, Receiver<Vec<f32>>) {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<AudioCommand>();
        // Bounded channel: ~30 seconds of audio at 16kHz in 1-second chunks
        let (audio_tx, audio_rx) = crossbeam_channel::bounded::<Vec<f32>>(30);

        std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || {
                let mut capture = AudioCapture {
                    stream: None,
                    audio_sender: audio_tx,
                };

                // Block on commands from main thread
                while let Ok(cmd) = cmd_rx.recv() {
                    match cmd {
                        AudioCommand::Start => {
                            if let Err(e) = capture.start() {
                                error!("Failed to start audio capture: {e}");
                            }
                        }
                        AudioCommand::Stop => {
                            capture.stop();
                        }
                    }
                }

                info!("Audio thread exiting");
            })
            .expect("Failed to spawn audio thread");

        (cmd_tx, audio_rx)
    }

    fn start(&mut self) -> Result<(), String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "No input device available".to_string())?;

        let device_name = device.name().unwrap_or_else(|_| "Unknown".into());
        info!(device = %device_name, "Using input device");

        let config = Self::preferred_config(&device)?;
        let sample_rate = config.sample_rate.0;
        let channels = config.channels;

        info!(
            sample_rate = sample_rate,
            channels = channels,
            "Audio capture config"
        );

        let mut resampler = Resampler::new(sample_rate, channels);
        let sender = self.audio_sender.clone();

        let err_fn = |err: cpal::StreamError| {
            error!("Audio stream error: {}", err);
        };

        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let resampled = resampler.process(data);
                    if !resampled.is_empty() {
                        if sender.try_send(resampled).is_err() {
                            warn!("Audio buffer full, dropping samples");
                        }
                    }
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Failed to build input stream: {e}"))?;

        stream
            .play()
            .map_err(|e| format!("Failed to start stream: {e}"))?;
        self.stream = Some(stream);

        info!("Audio capture started");
        Ok(())
    }

    fn stop(&mut self) {
        if self.stream.take().is_some() {
            info!("Audio capture stopped");
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
