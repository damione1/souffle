//! Native playback for a meeting's recorded audio (SOU-187 milestone 7,
//! AC14/AC16) - genuinely new engineering, not a port: the Svelte
//! `MeetingAudioPlayerSection` just delegates to an HTML `<audio>` element,
//! which has no Rust equivalent. Decodes the whole file up front via
//! `souffle_lib::audio::recorder::decode_ogg_opus` and plays it back through
//! a `cpal` output stream driven by a shared sample position.
//!
//! SOU-258: [`load`] is slow (decode, resample to the output device rate,
//! open the output stream: seconds for a long meeting or a Bluetooth
//! output) and therefore never runs on the UI thread - `main.rs` calls it
//! on a worker and hands the finished [`AudioPlayer`] (cpal's macOS stream
//! is `Send`) back through `invoke_from_event_loop`. What the page needs at
//! once, the waveform and the duration, comes from
//! [`cached_summary`], a small sidecar written the first time a recording
//! is decoded.
//!
//! The decoded buffer is always a 48kHz **mono downmix** of whatever channel
//! layout the file has: a legacy mono recording as-is, a diarized stereo
//! recording (left = you, right = the other participants) folded into one
//! "merged meeting" mix. No lane selector is exposed here.
//!
//! Known limitation, not solved here: the entire recording is held decoded
//! in memory as f32 (48kHz mono is ~192KB/s), which is fine for a typical
//! meeting but would be a real memory cost for a multi-hour one. Streaming
//! decode+playback would need a ring buffer and a decoder thread instead of
//! this eager, single-shot decode - real extra engineering, deliberately
//! deferred rather than half-built.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use souffle_lib::audio::recorder::{
    WaveformSummary, cached_waveform_summary, decode_ogg_opus, store_waveform_summary,
    waveform_peaks, waveform_source,
};
use souffle_lib::audio::resampler::Resampler;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Number of waveform bars shown - a fixed overview resolution, independent
/// of recording length.
const WAVEFORM_BUCKETS: usize = 200;

pub struct AudioPlayer {
    samples: Arc<Vec<f32>>,
    sample_rate: u32,
    position: Arc<AtomicUsize>,
    playing: Arc<AtomicBool>,
    // Kept alive for as long as the player exists; dropping it stops the
    // output stream. Never read directly after construction.
    _stream: cpal::Stream,
}

impl AudioPlayer {
    pub fn play(&self) {
        self.playing.store(true, Ordering::Relaxed);
    }

    pub fn pause(&self) {
        self.playing.store(false, Ordering::Relaxed);
    }

    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    pub fn seek_to(&self, fraction: f32) {
        self.position
            .store(seek_index(fraction, self.samples.len()), Ordering::Relaxed);
    }

    /// Seeks to an absolute position, e.g. a transcript paragraph's
    /// `start_time`. Naive for a meeting with more than one recording
    /// session (only the first session's audio is ever loaded, see
    /// `load`'s doc comment) - correct for the common single-session case.
    pub fn seek_to_seconds(&self, seconds: f64) {
        let index = (seconds.max(0.0) * f64::from(self.sample_rate)) as usize;
        self.position
            .store(index.min(self.samples.len()), Ordering::Relaxed);
    }

    pub fn position_seconds(&self) -> f64 {
        seconds_from_index(self.position.load(Ordering::Relaxed), self.sample_rate)
    }

    pub fn duration_seconds(&self) -> f64 {
        seconds_from_index(self.samples.len(), self.sample_rate)
    }

    pub fn progress(&self) -> f32 {
        progress_from_index(self.position.load(Ordering::Relaxed), self.samples.len())
    }
}

/// Sample index `fraction` (0.0..=1.0, clamped) maps to within a track of
/// `len` samples. Pure so it can be unit tested without a real audio
/// device - `AudioPlayer` itself always opens one, even paused, so it can't
/// be constructed in a portable test.
fn seek_index(fraction: f32, len: usize) -> usize {
    let fraction = fraction.clamp(0.0, 1.0);
    ((fraction as f64) * len as f64) as usize
}

fn seconds_from_index(index: usize, sample_rate: u32) -> f64 {
    index as f64 / f64::from(sample_rate)
}

fn progress_from_index(index: usize, len: usize) -> f32 {
    if len == 0 {
        return 0.0;
    }
    index as f32 / len as f32
}

/// `load`'s error when `still_wanted` said no: not a failure to report.
pub const LOAD_ABANDONED: &str = "audio load abandoned";

/// Sample rate `decode_ogg_opus` always returns.
const DECODE_RATE: u32 = 48_000;

/// The waveform + duration cached for `path` by an earlier [`load`], if the
/// file has not changed since. One `stat` and a small read: UI-thread safe.
pub fn cached_summary(path: &Path) -> Option<WaveformSummary> {
    cached_waveform_summary(path, WAVEFORM_BUCKETS)
}

/// Decodes `path` and opens a ready-to-play output stream (paused) plus its
/// waveform summary, which it also caches next to the recording for the
/// next open. Slow (see the module doc): call it off the UI thread.
/// `still_wanted` is asked between the expensive stages, so a load the user
/// has already navigated away from stops early instead of burning a core.
/// The stream is created and started immediately so the first `play()` has
/// no extra latency, but playback only advances once `play()` is called -
/// the callback outputs silence until then.
pub fn load(
    path: &Path,
    still_wanted: impl Fn() -> bool,
) -> Result<(AudioPlayer, WaveformSummary), String> {
    // Stamped before decoding: if the file changes meanwhile, the cache is
    // not written under the new stamp with the old content's peaks.
    let decoded_from = waveform_source(path);
    let decoded = decode_ogg_opus(path)?;
    let summary = match cached_summary(path) {
        Some(summary) => summary,
        None => {
            let summary = WaveformSummary {
                peaks: waveform_peaks(&decoded, WAVEFORM_BUCKETS),
                duration_seconds: seconds_from_index(decoded.len(), DECODE_RATE),
            };
            if let Some(decoded_from) = &decoded_from
                && let Err(e) =
                    store_waveform_summary(path, WAVEFORM_BUCKETS, &summary, decoded_from)
            {
                eprintln!("Waveform cache not written: {e}");
            }
            summary
        }
    };

    if !still_wanted() {
        return Err(LOAD_ABANDONED.into());
    }

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("Aucun p\u{e9}riph\u{e9}rique de sortie audio")?;
    let config = device
        .default_output_config()
        .map_err(|e| format!("Config de sortie audio: {e}"))?;
    let device_rate = config.sample_rate();
    let channels = config.channels() as usize;

    let samples = if device_rate == DECODE_RATE {
        decoded
    } else {
        let mut resampler = Resampler::new(DECODE_RATE, 1, device_rate, 1.0);
        let mut out = resampler.process(&decoded);
        out.extend(resampler.flush());
        out
    };
    if !still_wanted() {
        return Err(LOAD_ABANDONED.into());
    }
    let samples = Arc::new(samples);
    let position = Arc::new(AtomicUsize::new(0));
    let playing = Arc::new(AtomicBool::new(false));

    let stream_samples = samples.clone();
    let stream_position = position.clone();
    let stream_playing = playing.clone();
    let err_fn = |e: cpal::StreamError| eprintln!("Erreur flux de sortie audio: {e}");

    // Only the two formats actually seen from `default_output_config()` on
    // macOS are handled; the rest of cpal::SampleFormat's many integer/DSD
    // variants would never come from this call, so listing them all out
    // would be noise, not safety.
    #[allow(clippy::wildcard_enum_match_arm)]
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &config.into(),
            move |data: &mut [f32], _| {
                fill_buffer(
                    data,
                    channels,
                    &stream_samples,
                    &stream_position,
                    &stream_playing,
                    |v| v,
                );
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_output_stream(
            &config.into(),
            move |data: &mut [i16], _| {
                fill_buffer(
                    data,
                    channels,
                    &stream_samples,
                    &stream_position,
                    &stream_playing,
                    |v| (v * f32::from(i16::MAX)) as i16,
                );
            },
            err_fn,
            None,
        ),
        other => {
            return Err(format!(
                "Format de sortie audio non support\u{e9}: {other:?}"
            ));
        }
    }
    .map_err(|e| format!("Cr\u{e9}er le flux de sortie: {e}"))?;

    stream
        .play()
        .map_err(|e| format!("D\u{e9}marrer le flux de sortie: {e}"))?;

    Ok((
        AudioPlayer {
            samples,
            sample_rate: device_rate,
            position,
            playing,
            _stream: stream,
        },
        summary,
    ))
}

/// SVG path commands drawing one bar per peak, centred on the middle line,
/// in a `peaks.len()` x 1 viewbox (`AudioPlayerSection` stretches it to the
/// strip). One `Path` item for the whole overview instead of a layout plus a
/// `Rectangle` per bar (SOU-258).
pub fn waveform_commands(peaks: &[f32]) -> String {
    use std::fmt::Write as _;
    const BAR_WIDTH: f32 = 0.7;
    // ~2px of the 56px strip: silence still reads as a flat line.
    const MIN_HEIGHT: f32 = 0.04;
    let mut commands = String::with_capacity(peaks.len() * 32);
    for (i, peak) in peaks.iter().enumerate() {
        let height = peak.clamp(MIN_HEIGHT, 1.0);
        let top = (1.0 - height) / 2.0;
        let _ = write!(
            commands,
            "M{i} {top:.3}h{BAR_WIDTH}v{height:.3}h-{BAR_WIDTH}z"
        );
    }
    commands
}

/// Writes one output callback's worth of interleaved audio: the current
/// mono sample repeated across every channel, silence (without advancing)
/// while paused or past the end.
fn fill_buffer<T: Copy>(
    data: &mut [T],
    channels: usize,
    samples: &[f32],
    position: &AtomicUsize,
    playing: &AtomicBool,
    convert: impl Fn(f32) -> T,
) {
    let is_playing = playing.load(Ordering::Relaxed);
    let mut pos = position.load(Ordering::Relaxed);
    let silence = convert(0.0);
    for frame in data.chunks_mut(channels.max(1)) {
        let sample = if is_playing && pos < samples.len() {
            convert(samples[pos])
        } else {
            silence
        };
        for out in frame.iter_mut() {
            *out = sample;
        }
        if is_playing && pos < samples.len() {
            pos += 1;
        }
    }
    if is_playing {
        let clamped = pos.min(samples.len());
        position.store(clamped, Ordering::Relaxed);
        if clamped >= samples.len() {
            playing.store(false, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seek_index_clamps_and_scales() {
        assert_eq!(seek_index(0.0, 100), 0);
        assert_eq!(seek_index(0.5, 100), 50);
        assert_eq!(seek_index(1.0, 100), 100);
        assert_eq!(seek_index(1.5, 100), 100);
        assert_eq!(seek_index(-1.0, 100), 0);
        assert_eq!(seek_index(0.5, 0), 0);
    }

    #[test]
    fn seconds_from_index_divides_by_sample_rate() {
        assert_eq!(seconds_from_index(48_000, 48_000), 1.0);
        assert_eq!(seconds_from_index(24_000, 48_000), 0.5);
        assert_eq!(seconds_from_index(0, 48_000), 0.0);
    }

    #[test]
    fn waveform_commands_draws_one_closed_bar_per_peak() {
        let commands = waveform_commands(&[1.0, 0.0]);
        assert_eq!(commands.matches('M').count(), 2);
        assert_eq!(commands.matches('z').count(), 2);
        // A full peak spans the whole height, silence keeps a thin line.
        assert!(commands.starts_with("M0 0.000h0.7v1.000h-0.7z"));
        assert!(commands.contains("M1 0.480h0.7v0.040h-0.7z"));
        assert!(waveform_commands(&[]).is_empty());
    }

    #[test]
    fn progress_from_index_normalizes_and_handles_empty() {
        assert_eq!(progress_from_index(50, 100), 0.5);
        assert_eq!(progress_from_index(100, 100), 1.0);
        assert_eq!(progress_from_index(0, 0), 0.0);
    }
}
