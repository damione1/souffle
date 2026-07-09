//! Short bundled WAV cues for dictation start/stop confirmation.

use std::path::Path;
use std::thread;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::WavReader;
use tracing::warn;

use crate::ort_runtime::resolve_resource;
use crate::settings::AppSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictationFeedbackKind {
    Start,
    Stop,
}

/// Fire-and-forget playback on a background thread so capture never blocks.
pub fn play_dictation_feedback(settings: &AppSettings, kind: DictationFeedbackKind) {
    if !settings.feedback_sounds_enabled {
        return;
    }
    let volume = normalized_volume(settings.feedback_sounds_volume);
    let filename = match kind {
        DictationFeedbackKind::Start => "sounds/dictation_start.wav",
        DictationFeedbackKind::Stop => "sounds/dictation_stop.wav",
    };
    let Some(path) = resolve_resource(filename) else {
        warn!("Feedback sound not found: {filename}");
        return;
    };
    thread::Builder::new()
        .name("feedback-sound".into())
        .spawn(move || {
            if let Err(e) = play_wav_blocking(&path, volume) {
                warn!("Feedback sound playback failed: {e}");
            }
        })
        .ok();
}

fn normalized_volume(percent: u32) -> f32 {
    (percent.min(100) as f32 / 100.0).clamp(0.0, 1.0)
}

fn play_wav_blocking(path: &Path, volume: f32) -> Result<(), String> {
    let mut reader = WavReader::open(path).map_err(|e| format!("Open WAV: {e}"))?;
    let spec = reader.spec();
    if spec.channels != 1 {
        return Err(format!("Expected mono WAV, got {} channels", spec.channels));
    }

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|s| s.map_err(|e| format!("Read sample: {e}")))
            .collect::<Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| {
                s.map(|v| v as f32 / i16::MAX as f32)
                    .map_err(|e| format!("Read sample: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?,
    };

    if samples.is_empty() {
        return Ok(());
    }

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "No default output device".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("Output config: {e}"))?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;

    let scaled: Vec<f32> = samples
        .iter()
        .map(|s| (s * volume).clamp(-1.0, 1.0))
        .collect();
    let playback = resample_linear(&scaled, spec.sample_rate, sample_rate);
    let interleaved: Vec<f32> = if channels == 1 {
        playback
    } else {
        playback
            .iter()
            .flat_map(|sample| std::iter::repeat_n(*sample, channels))
            .collect()
    };

    let config = cpal::StreamConfig {
        channels: supported.channels(),
        sample_rate: supported.sample_rate(),
        buffer_size: cpal::BufferSize::Default,
    };

    let sample_count = interleaved.len();
    let mut index = 0usize;
    let stream = device
        .build_output_stream(
            &config,
            move |out: &mut [f32], _| {
                for frame in out.chunks_mut(channels) {
                    if index < interleaved.len() {
                        let sample = interleaved[index];
                        for slot in frame.iter_mut() {
                            *slot = sample;
                        }
                        index += 1;
                    } else {
                        for slot in frame.iter_mut() {
                            *slot = 0.0;
                        }
                    }
                }
            },
            move |e| warn!("Feedback output stream error: {e}"),
            None,
        )
        .map_err(|e| format!("Build output stream: {e}"))?;

    stream.play().map_err(|e| format!("Play stream: {e}"))?;
    let duration = duration_from_samples(sample_count, channels, sample_rate);
    thread::sleep(duration + std::time::Duration::from_millis(30));
    Ok(())
}

fn duration_from_samples(samples: usize, channels: usize, rate: u32) -> std::time::Duration {
    let frames = samples / channels.max(1);
    std::time::Duration::from_secs_f64(frames as f64 / rate as f64)
}

fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }
    let out_len = ((input.len() as f64) * to_rate as f64 / from_rate as f64).ceil() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 * from_rate as f64 / to_rate as f64;
        let idx = src_pos.floor() as usize;
        let frac = (src_pos - idx as f64) as f32;
        let a = input[idx.min(input.len() - 1)];
        let b = input[(idx + 1).min(input.len() - 1)];
        out.push(a + (b - a) * frac);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_volume_clamps() {
        assert_eq!(normalized_volume(0), 0.0);
        assert_eq!(normalized_volume(100), 1.0);
        assert_eq!(normalized_volume(200), 1.0);
    }

    #[test]
    fn resample_same_rate_is_copy() {
        let input = vec![0.1, 0.2, 0.3];
        assert_eq!(resample_linear(&input, 44_100, 44_100), input);
    }
}
