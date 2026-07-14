//! Pure-Rust Kaldi-compatible fbank frontend for the WeSpeaker embedding
//! model (80 mel bins, 16kHz, matching `feats` in
//! `wespeaker_en_voxceleb_resnet34_LM.onnx`).
//!
//! Implemented from scratch rather than binding `kaldi-native-fbank` (a C++
//! library) to avoid adding a second native build toolchain to this project;
//! `rustfft` is already resolved in the dependency tree via `rubato`. This
//! matches Kaldi's own defaults for fbank extraction as used by
//! WeSpeaker/WeNet training (see sherpa-onnx's `FeatureExtractorConfig`:
//! snip_edges=false, dither=0, frame_length_ms=25, frame_shift_ms=10,
//! povey window, per-utterance cepstral mean normalization) but is not
//! guaranteed to be bit-exact with Kaldi: frame-boundary reflection padding
//! and floating point order of operations can differ by noise-level amounts,
//! which does not materially affect embedding-based clustering.

use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

pub const NUM_MEL_BINS: usize = 80;
const FRAME_LENGTH_MS: f32 = 25.0;
const FRAME_SHIFT_MS: f32 = 10.0;
const PREEMPHASIS_COEFF: f32 = 0.97;
const LOW_FREQ_HZ: f32 = 20.0;

/// Number of frames Kaldi's snip_edges=false framing produces for
/// `num_samples` samples at a `frame_shift`-sample step: frames are centered
/// so the signal is covered edge to edge, rather than only fully-contained
/// windows being kept.
fn num_frames(num_samples: usize, frame_shift: usize) -> usize {
    if num_samples == 0 || frame_shift == 0 {
        return 0;
    }
    (num_samples as f64 / frame_shift as f64).round() as usize
}

/// Extract one frame, reflecting sample indices that fall outside
/// `[0, samples.len())` back into range (Kaldi's `Reflect` edge handling for
/// snip_edges=false, which lets the first/last frame center on the signal
/// boundary instead of being dropped).
fn extract_frame(samples: &[f32], frame_index: usize, frame_shift: usize, frame_length: usize) -> Vec<f32> {
    let num_samples = samples.len() as i64;
    let midpoint = (frame_shift as i64) * (frame_index as i64) + (frame_shift as i64) / 2;
    let start = midpoint - (frame_length as i64) / 2;

    let mut frame = Vec::with_capacity(frame_length);
    for i in 0..frame_length as i64 {
        let mut s = start + i;
        while s < 0 || s >= num_samples {
            s = if s < 0 { -s - 1 } else { 2 * num_samples - 1 - s };
        }
        frame.push(samples[s as usize]);
    }
    frame
}

fn remove_dc_offset(frame: &mut [f32]) {
    let mean = frame.iter().sum::<f32>() / frame.len() as f32;
    for s in frame.iter_mut() {
        *s -= mean;
    }
}

fn preemphasize(frame: &mut [f32], coeff: f32) {
    for i in (1..frame.len()).rev() {
        frame[i] -= coeff * frame[i - 1];
    }
    frame[0] -= coeff * frame[0];
}

/// Kaldi's "povey" analysis window (the default for fbank/mfcc extraction).
fn povey_window(len: usize) -> Vec<f32> {
    if len <= 1 {
        return vec![1.0; len];
    }
    (0..len)
        .map(|i| {
            let x = (2.0 * std::f32::consts::PI * i as f32) / (len as f32 - 1.0);
            (0.5 - 0.5 * x.cos()).powf(0.85)
        })
        .collect()
}

fn mel_scale(freq_hz: f32) -> f32 {
    1127.0 * (1.0 + freq_hz / 700.0).ln()
}

/// Kaldi-style triangular mel filterbank: `num_bins` overlapping triangles
/// linearly spaced in mel scale between `low_freq_hz` and Nyquist, applied to
/// an `fft_size`-point real power spectrum.
fn build_mel_filterbank(num_bins: usize, fft_size: usize, sample_rate: u32, low_freq_hz: f32) -> Vec<Vec<f32>> {
    let high_freq_hz = sample_rate as f32 / 2.0;
    let mel_low = mel_scale(low_freq_hz);
    let mel_high = mel_scale(high_freq_hz);
    let mel_delta = (mel_high - mel_low) / (num_bins as f32 + 1.0);

    let num_fft_bins = fft_size / 2 + 1;
    let mut filters = vec![vec![0.0f32; num_fft_bins]; num_bins];

    for (bin, filter) in filters.iter_mut().enumerate() {
        let left = mel_low + bin as f32 * mel_delta;
        let center = mel_low + (bin as f32 + 1.0) * mel_delta;
        let right = mel_low + (bin as f32 + 2.0) * mel_delta;

        for (k, weight) in filter.iter_mut().enumerate() {
            let freq = k as f32 * sample_rate as f32 / fft_size as f32;
            let mel = mel_scale(freq);
            if mel > left && mel < right {
                *weight = if mel <= center {
                    (mel - left) / (center - left)
                } else {
                    (right - mel) / (right - center)
                };
            }
        }
    }
    filters
}

/// Subtract the per-column (per mel bin) mean across all frames: the
/// per-utterance cepstral mean normalization the embedding model was trained
/// with (see `SubtractGlobalMean` in sherpa-onnx's embedding extractor).
fn apply_cepstral_mean_normalization(rows: &mut [Vec<f32>]) {
    let Some(dim) = rows.first().map(|r| r.len()) else {
        return;
    };
    let mut mean = vec![0.0f32; dim];
    for row in rows.iter() {
        for (m, v) in mean.iter_mut().zip(row.iter()) {
            *m += v;
        }
    }
    let n = rows.len() as f32;
    for m in mean.iter_mut() {
        *m /= n;
    }
    for row in rows.iter_mut() {
        for (v, m) in row.iter_mut().zip(mean.iter()) {
            *v -= m;
        }
    }
}

pub struct FbankExtractor {
    frame_length: usize,
    frame_shift: usize,
    fft_size: usize,
    window: Vec<f32>,
    mel_filters: Vec<Vec<f32>>,
    fft: Arc<dyn Fft<f32>>,
}

impl FbankExtractor {
    pub fn new(sample_rate: u32) -> Self {
        let frame_length = ((FRAME_LENGTH_MS / 1000.0) * sample_rate as f32).round() as usize;
        let frame_shift = ((FRAME_SHIFT_MS / 1000.0) * sample_rate as f32).round() as usize;
        let fft_size = frame_length.next_power_of_two();
        let window = povey_window(frame_length);
        let mel_filters = build_mel_filterbank(NUM_MEL_BINS, fft_size, sample_rate, LOW_FREQ_HZ);
        let fft = FftPlanner::<f32>::new().plan_fft_forward(fft_size);

        Self {
            frame_length,
            frame_shift,
            fft_size,
            window,
            mel_filters,
            fft,
        }
    }

    /// Compute `NUM_MEL_BINS`-dim log-mel filterbank features, one row per
    /// frame, cepstral-mean-normalized across the whole input. Empty input
    /// yields no rows.
    pub fn compute(&self, samples: &[f32]) -> Vec<Vec<f32>> {
        let frame_count = num_frames(samples.len(), self.frame_shift);
        let mut rows = Vec::with_capacity(frame_count);

        for f in 0..frame_count {
            let mut frame = extract_frame(samples, f, self.frame_shift, self.frame_length);

            remove_dc_offset(&mut frame);
            preemphasize(&mut frame, PREEMPHASIS_COEFF);
            for (s, w) in frame.iter_mut().zip(self.window.iter()) {
                *s *= w;
            }

            let mut spectrum: Vec<Complex32> = frame.iter().map(|&s| Complex32::new(s, 0.0)).collect();
            spectrum.resize(self.fft_size, Complex32::new(0.0, 0.0));
            self.fft.process(&mut spectrum);

            let power: Vec<f32> = spectrum[..self.fft_size / 2 + 1].iter().map(|c| c.norm_sqr()).collect();

            let mel_energies: Vec<f32> = self
                .mel_filters
                .iter()
                .map(|filter| {
                    let energy: f32 = filter.iter().zip(power.iter()).map(|(w, p)| w * p).sum();
                    energy.max(f32::MIN_POSITIVE).ln()
                })
                .collect();

            rows.push(mel_energies);
        }

        apply_cepstral_mean_normalization(&mut rows);
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_wave(sample_rate: u32, duration_s: f32, freq_hz: f32) -> Vec<f32> {
        let n = (sample_rate as f32 * duration_s) as usize;
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq_hz * i as f32 / sample_rate as f32).sin())
            .collect()
    }

    #[test]
    fn num_frames_matches_round_of_samples_over_shift() {
        assert_eq!(num_frames(16_000, 160), 100);
        assert_eq!(num_frames(0, 160), 0);
        assert_eq!(num_frames(80, 160), 1); // rounds up from 0.5
    }

    #[test]
    fn mel_scale_is_monotonically_increasing() {
        assert!(mel_scale(100.0) < mel_scale(1000.0));
        assert!(mel_scale(1000.0) < mel_scale(8000.0));
        assert_eq!(mel_scale(0.0), 0.0);
    }

    #[test]
    fn extract_frame_reflects_at_left_boundary() {
        let samples = vec![10.0, 20.0, 30.0, 40.0, 50.0];
        // frame_index 0, shift 2, length 4 -> midpoint=1, start=1-2=-1
        let frame = extract_frame(&samples, 0, 2, 4);
        assert_eq!(frame.len(), 4);
        // index -1 reflects to 0
        assert_eq!(frame[0], samples[0]);
    }

    #[test]
    fn compute_output_shape_matches_frame_count_and_mel_bins() {
        let extractor = FbankExtractor::new(16_000);
        let samples = sine_wave(16_000, 1.0, 440.0);
        let features = extractor.compute(&samples);
        assert_eq!(features.len(), num_frames(samples.len(), 160));
        for row in &features {
            assert_eq!(row.len(), NUM_MEL_BINS);
        }
    }

    #[test]
    fn compute_empty_input_yields_no_frames() {
        let extractor = FbankExtractor::new(16_000);
        assert!(extractor.compute(&[]).is_empty());
    }

    #[test]
    fn compute_silence_produces_finite_values_no_nan() {
        let extractor = FbankExtractor::new(16_000);
        let samples = vec![0.0f32; 16_000];
        let features = extractor.compute(&samples);
        assert!(!features.is_empty());
        for row in &features {
            for &v in row {
                assert!(v.is_finite());
            }
        }
    }

    #[test]
    fn compute_is_deterministic() {
        let extractor = FbankExtractor::new(16_000);
        let samples = sine_wave(16_000, 0.5, 220.0);
        let a = extractor.compute(&samples);
        let b = extractor.compute(&samples);
        assert_eq!(a, b);
    }

    #[test]
    fn compute_applies_cepstral_mean_normalization() {
        let extractor = FbankExtractor::new(16_000);
        let samples = sine_wave(16_000, 1.0, 300.0);
        let features = extractor.compute(&samples);
        let dim = NUM_MEL_BINS;
        for col in 0..dim {
            let mean: f32 = features.iter().map(|r| r[col]).sum::<f32>() / features.len() as f32;
            assert!(mean.abs() < 1e-3, "column {col} mean {mean} not ~0");
        }
    }
}
