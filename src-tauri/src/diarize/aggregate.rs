//! Sliding-window aggregation of per-frame speaker activity onto a single
//! timeline, and hysteresis binarization of the resulting curves into
//! segments.
//!
//! The segmentation model is run on overlapping 10s windows (1s step); each
//! window independently emits activity scores at its own local frame
//! resolution. This module merges those overlapping windows into one
//! continuous per-local-speaker score curve (by averaging every window's
//! contribution to a given global frame; see `ComputeSpeakersPerFrame` in
//! sherpa-onnx's pyannote implementation), then converts each speaker's
//! curve into discrete segments with onset/offset hysteresis, exactly like
//! pyannote-audio's `Binarize`.

/// Average every window's per-frame, per-speaker soft activity score onto a
/// single global frame timeline. `window_scores[w][f][s]` is the score for
/// local speaker `s` at local frame `f` of window `w`. Windows are assumed to
/// start `window_shift_samples` apart and each local frame advances by
/// `receptive_field_shift_samples`.
///
/// Returns `curve[s]`, one continuous score sequence per local speaker slot,
/// sampled every `receptive_field_shift_samples`.
pub fn aggregate_windows(
    window_scores: &[Vec<Vec<f32>>],
    window_shift_samples: u64,
    receptive_field_shift_samples: u64,
    num_speakers: usize,
) -> Vec<Vec<f32>> {
    if window_scores.is_empty() {
        return vec![Vec::new(); num_speakers];
    }

    let frames_per_window = window_scores[0].len();
    let num_windows = window_scores.len();

    let last_window_start_frame = ((num_windows - 1) as f64 * window_shift_samples as f64
        / receptive_field_shift_samples as f64)
        .round() as usize;
    let num_global_frames = last_window_start_frame + frames_per_window;

    let mut sum = vec![vec![0.0f32; num_global_frames]; num_speakers];
    let mut count = vec![0.0f32; num_global_frames];

    for (w, window) in window_scores.iter().enumerate() {
        let start_frame = (w as f64 * window_shift_samples as f64
            / receptive_field_shift_samples as f64)
            .round() as usize;

        for (local_frame, speaker_scores) in window.iter().enumerate() {
            let global_frame = start_frame + local_frame;
            if global_frame >= num_global_frames {
                continue;
            }
            for (s, &score) in speaker_scores.iter().enumerate() {
                sum[s][global_frame] += score;
            }
            count[global_frame] += 1.0;
        }
    }

    for s in sum.iter_mut() {
        for (frame, value) in s.iter_mut().enumerate() {
            let c = count[frame].max(1e-12);
            *value /= c;
        }
    }

    sum
}

/// Onset/offset hysteresis binarization + gap merge + short-segment pruning,
/// applied to one speaker's continuous activity curve.
#[derive(Debug, Clone, Copy)]
pub struct BinarizeParams {
    pub onset: f32,
    pub offset: f32,
    pub min_duration_on_s: f64,
    pub min_duration_off_s: f64,
}

impl Default for BinarizeParams {
    fn default() -> Self {
        Self {
            onset: 0.5,
            offset: 0.5,
            min_duration_on_s: 0.1,
            min_duration_off_s: 0.5,
        }
    }
}

/// Convert a per-frame activity curve into `(start_s, end_s)` speech
/// segments: a segment starts once the score crosses `onset` and ends once it
/// drops below `offset` (hysteresis: `offset <= onset` avoids flicker on a
/// noisy curve hovering near a single threshold). Segments separated by a gap
/// shorter than `min_duration_off_s` are merged; segments shorter than
/// `min_duration_on_s` after merging are dropped.
pub fn binarize_curve(
    curve: &[f32],
    frame_duration_s: f64,
    frame_offset_s: f64,
    params: &BinarizeParams,
) -> Vec<(f64, f64)> {
    let mut raw = Vec::new();
    let mut active = false;
    let mut start_frame = 0usize;

    for (i, &score) in curve.iter().enumerate() {
        if !active && score >= params.onset {
            active = true;
            start_frame = i;
        } else if active && score < params.offset {
            active = false;
            raw.push((start_frame, i));
        }
    }
    if active {
        raw.push((start_frame, curve.len()));
    }

    let mut segments: Vec<(f64, f64)> = raw
        .into_iter()
        .map(|(s, e)| {
            (
                s as f64 * frame_duration_s + frame_offset_s,
                e as f64 * frame_duration_s + frame_offset_s,
            )
        })
        .collect();

    merge_close_segments(&mut segments, params.min_duration_off_s);
    segments.retain(|(s, e)| e - s >= params.min_duration_on_s);
    segments
}

/// Merge consecutive segments whose gap is smaller than `min_gap_s`. Assumes
/// `segments` is already sorted by start time (true for `binarize_curve`'s
/// single-pass scan output).
fn merge_close_segments(segments: &mut Vec<(f64, f64)>, min_gap_s: f64) {
    if segments.len() < 2 {
        return;
    }
    let mut merged = Vec::with_capacity(segments.len());
    let mut current = segments[0];
    for &(start, end) in &segments[1..] {
        if start - current.1 < min_gap_s {
            current.1 = current.1.max(end);
        } else {
            merged.push(current);
            current = (start, end);
        }
    }
    merged.push(current);
    *segments = merged;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_single_window_passes_through() {
        let window_scores = vec![vec![vec![0.9, 0.1], vec![0.8, 0.2]]];
        let curve = aggregate_windows(&window_scores, 16_000, 270, 2);
        assert_eq!(curve[0], vec![0.9, 0.8]);
        assert_eq!(curve[1], vec![0.1, 0.2]);
    }

    #[test]
    fn aggregate_averages_overlapping_windows() {
        // Two windows shifted by exactly one frame; overlap region averages.
        let window_scores = vec![
            vec![vec![1.0], vec![1.0], vec![0.0]],
            vec![vec![1.0], vec![0.0], vec![0.0]],
        ];
        let curve = aggregate_windows(&window_scores, 270, 270, 1);
        // global frame 1 is window0.frame1 (1.0) and window1.frame0 (1.0) -> 1.0
        // global frame 2 is window0.frame2 (0.0) and window1.frame1 (0.0) -> 0.0
        assert_eq!(curve[0].len(), 4);
        assert!((curve[0][0] - 1.0).abs() < 1e-6);
        assert!((curve[0][1] - 1.0).abs() < 1e-6);
        assert!((curve[0][2] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn aggregate_empty_input_returns_empty_curves() {
        let curve = aggregate_windows(&[], 16_000, 270, 3);
        assert_eq!(curve.len(), 3);
        assert!(curve.iter().all(|c| c.is_empty()));
    }

    fn params() -> BinarizeParams {
        BinarizeParams {
            onset: 0.5,
            offset: 0.5,
            min_duration_on_s: 0.0,
            min_duration_off_s: 0.0,
        }
    }

    #[test]
    fn binarize_single_burst_above_threshold() {
        let curve = vec![0.0, 0.0, 0.9, 0.9, 0.9, 0.0, 0.0];
        let segs = binarize_curve(&curve, 1.0, 0.0, &params());
        assert_eq!(segs, vec![(2.0, 5.0)]);
    }

    #[test]
    fn binarize_curve_never_crossing_onset_yields_nothing() {
        let curve = vec![0.1, 0.2, 0.3, 0.1];
        let segs = binarize_curve(&curve, 1.0, 0.0, &params());
        assert!(segs.is_empty());
    }

    #[test]
    fn binarize_curve_active_through_end_closes_at_last_frame() {
        let curve = vec![0.0, 0.9, 0.9, 0.9];
        let segs = binarize_curve(&curve, 1.0, 0.0, &params());
        assert_eq!(segs, vec![(1.0, 4.0)]);
    }

    #[test]
    fn binarize_applies_frame_offset() {
        let curve = vec![0.9, 0.9];
        let segs = binarize_curve(&curve, 0.01, 0.03, &params());
        assert_eq!(segs, vec![(0.03, 0.05)]);
    }

    #[test]
    fn binarize_drops_short_segments_below_min_duration_on() {
        let curve = vec![0.0, 0.9, 0.0, 0.0, 0.9, 0.9, 0.9, 0.9, 0.0];
        let p = BinarizeParams {
            min_duration_on_s: 2.5,
            ..params()
        };
        let segs = binarize_curve(&curve, 1.0, 0.0, &p);
        // First burst is 1 frame (too short); second is 4 frames (kept).
        assert_eq!(segs, vec![(4.0, 8.0)]);
    }

    #[test]
    fn binarize_merges_gaps_below_min_duration_off() {
        let curve = vec![0.9, 0.0, 0.9, 0.9];
        let p = BinarizeParams {
            min_duration_off_s: 1.5,
            ..params()
        };
        let segs = binarize_curve(&curve, 1.0, 0.0, &p);
        assert_eq!(segs, vec![(0.0, 4.0)]);
    }

    #[test]
    fn binarize_keeps_gaps_at_or_above_min_duration_off() {
        let curve = vec![0.9, 0.0, 0.0, 0.9];
        let p = BinarizeParams {
            min_duration_off_s: 2.0,
            ..params()
        };
        let segs = binarize_curve(&curve, 1.0, 0.0, &p);
        assert_eq!(segs, vec![(0.0, 1.0), (3.0, 4.0)]);
    }

    #[test]
    fn binarize_empty_curve_yields_no_segments() {
        assert!(binarize_curve(&[], 1.0, 0.0, &params()).is_empty());
    }
}
