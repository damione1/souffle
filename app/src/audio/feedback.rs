//! Short bundled WAV cues for dictation start/stop confirmation.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
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

/// Cues currently on a playback thread. Each cue is 120 ms of audio plus the
/// time it takes to open the output, so several at once means threads are
/// piling up rather than overlapping.
static FEEDBACK_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// How many playback threads may exist at once. `play()` and
/// `build_output_stream` are synchronous CoreAudio calls with no deadline of
/// their own — the same property that let a wedged input device stall a
/// meeting for 72 s (SOU-125) — and this thread is never joined, so a wedged
/// output device would otherwise leave one stuck thread behind per dictation
/// toggle for the life of the app.
///
/// Four rather than two: a cue outlives its 120 ms by however long the
/// device takes to open, which on a Bluetooth sink waking from idle is not
/// negligible, and push-to-talk can fire start and stop inside that window.
/// The cap exists to bound a wedge, not to serialise normal use. Once it is
/// reached every further cue is skipped until a thread returns, which on a
/// genuinely wedged output is the honest outcome — a device that cannot
/// start cannot play a sound either.
const MAX_FEEDBACK_THREADS: usize = 4;

/// Take one of `max` slots, or report that none is free. Never blocks, and
/// never overshoots under concurrent callers.
fn try_reserve_slot(in_flight: &AtomicUsize, max: usize) -> bool {
    in_flight
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            (current < max).then_some(current + 1)
        })
        .is_ok()
}

/// Releases its slot however the playback thread ends, panic included.
struct FeedbackSlot(&'static AtomicUsize);

impl Drop for FeedbackSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
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
    if !try_reserve_slot(&FEEDBACK_IN_FLIGHT, MAX_FEEDBACK_THREADS) {
        // Earlier cues have not come back. The output device is not
        // answering; skip this one rather than stacking another thread that
        // may never return.
        warn!("Feedback sound skipped: previous playback has not finished");
        return;
    }
    let spawned = thread::Builder::new()
        .name("feedback-sound".into())
        .spawn(move || {
            let _slot = FeedbackSlot(&FEEDBACK_IN_FLIGHT);
            if let Err(e) = play_wav_blocking(&path, volume) {
                warn!("Feedback sound playback failed: {e}");
            }
        });
    if spawned.is_err() {
        FEEDBACK_IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
    }
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
    let sample_rate = supported.sample_rate();
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
    let _ = stream.pause();
    drop(stream);
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

    /// A wedged output device never returns from `play()`, and the playback
    /// thread is never joined. The cap is what keeps one stuck thread per
    /// dictation toggle from piling up for the life of the app.
    #[test]
    fn playback_slots_are_capped_and_returned() {
        static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
        assert!(try_reserve_slot(&IN_FLIGHT, 2));
        assert!(try_reserve_slot(&IN_FLIGHT, 2));
        assert!(
            !try_reserve_slot(&IN_FLIGHT, 2),
            "a third cue must be skipped, not stacked on a device that is not answering"
        );

        drop(FeedbackSlot(&IN_FLIGHT));
        assert!(
            try_reserve_slot(&IN_FLIGHT, 2),
            "a finished playback must free its slot"
        );
    }

    /// The slot is released however the playback thread ends: a panic inside
    /// cpal would otherwise burn one permanently, and four of them would
    /// silence the cues for the life of the app.
    #[test]
    fn a_panicking_playback_still_frees_its_slot() {
        static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
        assert!(try_reserve_slot(&IN_FLIGHT, 1));
        let panicked = std::panic::catch_unwind(|| {
            let _slot = FeedbackSlot(&IN_FLIGHT);
            panic!("cpal gave up mid-playback");
        });
        assert!(panicked.is_err());
        assert_eq!(IN_FLIGHT.load(Ordering::Acquire), 0);
        assert!(try_reserve_slot(&IN_FLIGHT, 1));
    }

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
