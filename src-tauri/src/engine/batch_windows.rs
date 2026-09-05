//! Shared batch-window cutting for Whisper and Parakeet.
//!
//! Both engines are stateless per window. Fixed 5 s non-overlapping cuts
//! split words at the boundary (`data | platform`, `Snow | flake`) and
//! push Parakeet into inventing a completion (`The next one is the same
//! thing.`). Cut on a silence gap in [4 s, 7 s] instead; if none, cut at
//! 7 s. The advertised pipeline hop stays 5 s — leftover samples sit in
//! the engine buffer.

/// Whisper / Parakeet sample rate.
pub const SAMPLE_RATE: u32 = 16_000;

/// Pipeline hop advertised as `chunk_size_samples`. Actor and CLI still
/// deliver this many samples per `transcribe` call.
pub const CHUNK_SAMPLES: usize = SAMPLE_RATE as usize * 5;

const MIN_CUT_SAMPLES: usize = SAMPLE_RATE as usize * 4;
const MAX_CUT_SAMPLES: usize = SAMPLE_RATE as usize * 7;

/// 10 ms frames at 16 kHz.
const FRAME_SAMPLES: usize = SAMPLE_RATE as usize / 100;
/// RMS below this in a 10 ms frame counts as silence.
const SILENCE_FRAME_RMS: f32 = 0.01;
/// Need this many consecutive silent frames (200 ms) to accept a gap.
const MIN_GAP_FRAMES: usize = 20;

pub fn pcm_rms(pcm: &[f32]) -> f32 {
    if pcm.is_empty() {
        return 0.0;
    }
    let sum: f32 = pcm.iter().map(|s| s * s).sum();
    (sum / pcm.len() as f32).sqrt()
}

fn frame_rms(frame: &[f32]) -> f32 {
    pcm_rms(frame)
}

/// How many samples to take from the front of `pcm` for the next inference
/// window. `None` = wait for more audio (buffer is short and has no gap).
pub fn find_cut_samples(pcm: &[f32]) -> Option<usize> {
    if pcm.len() < MIN_CUT_SAMPLES {
        return None;
    }

    let search_end = pcm.len().min(MAX_CUT_SAMPLES);
    if let Some(cut) = find_silence_cut(pcm, MIN_CUT_SAMPLES, search_end) {
        return Some(cut);
    }

    if pcm.len() >= MAX_CUT_SAMPLES {
        return Some(MAX_CUT_SAMPLES);
    }

    None
}

/// First 200 ms silence run whose start sits in `[search_start, search_end)`.
/// Returns the sample index at the start of the gap.
fn find_silence_cut(pcm: &[f32], search_start: usize, search_end: usize) -> Option<usize> {
    if FRAME_SAMPLES == 0 || search_end <= search_start {
        return None;
    }

    let mut run = 0usize;
    let mut run_start = search_start;
    let mut i = search_start;

    while i + FRAME_SAMPLES <= search_end {
        let frame = &pcm[i..i + FRAME_SAMPLES];
        if frame_rms(frame) < SILENCE_FRAME_RMS {
            if run == 0 {
                run_start = i;
            }
            run += 1;
            if run >= MIN_GAP_FRAMES {
                return Some(run_start.max(1));
            }
        } else {
            run = 0;
        }
        i += FRAME_SAMPLES;
    }

    None
}

/// Drain every ready inference window from the front of `buffer`.
pub fn drain_ready_windows(buffer: &mut Vec<f32>) -> Vec<Vec<f32>> {
    let mut windows = Vec::new();
    while let Some(cut) = find_cut_samples(buffer) {
        let cut = cut.min(buffer.len());
        if cut == 0 {
            break;
        }
        windows.push(buffer.drain(..cut).collect());
    }
    windows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(seconds: f64, amplitude: f32) -> Vec<f32> {
        let n = (seconds * SAMPLE_RATE as f64).round() as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / SAMPLE_RATE as f32;
                (t * 440.0 * 2.0 * std::f32::consts::PI).sin() * amplitude
            })
            .collect()
    }

    fn silence(seconds: f64) -> Vec<f32> {
        vec![0.0; (seconds * SAMPLE_RATE as f64).round() as usize]
    }

    #[test]
    fn continuous_speech_does_not_cut_at_five_seconds() {
        let pcm = sine(5.0, 0.3);
        assert_eq!(
            find_cut_samples(&pcm),
            None,
            "5 s of speech with no gap must wait — cutting here splits data|platform"
        );
    }

    #[test]
    fn continuous_speech_hard_caps_at_seven_seconds() {
        let pcm = sine(8.0, 0.3);
        assert_eq!(find_cut_samples(&pcm), Some(MAX_CUT_SAMPLES));
        let mut buf = pcm;
        let windows = drain_ready_windows(&mut buf);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].len(), MAX_CUT_SAMPLES);
        assert_eq!(buf.len(), SAMPLE_RATE as usize); // 1 s leftover
    }

    #[test]
    fn silence_gap_in_four_to_seven_is_preferred_cut() {
        let mut pcm = sine(4.5, 0.3);
        pcm.extend(silence(0.4));
        pcm.extend(sine(2.0, 0.3));
        let cut = find_cut_samples(&pcm).expect("gap at 4.5 s");
        let start = (4.5 * SAMPLE_RATE as f64) as usize;
        let end = (4.9 * SAMPLE_RATE as f64) as usize;
        assert!(
            (start..end).contains(&cut),
            "cut {cut} should sit in the 4.5–4.9 s gap"
        );
    }

    #[test]
    fn short_buffer_waits() {
        let pcm = sine(3.5, 0.3);
        assert_eq!(find_cut_samples(&pcm), None);
        assert!(drain_ready_windows(&mut pcm.clone()).is_empty());
    }

    #[test]
    fn five_second_boundary_stays_inside_one_window() {
        // Speech through the old 5 s knife-edge, then a gap at 6 s.
        let mut pcm = sine(6.0, 0.3);
        pcm.extend(silence(0.3));
        pcm.extend(sine(1.5, 0.3));
        let cut = find_cut_samples(&pcm).expect("gap at 6 s");
        let five = CHUNK_SAMPLES;
        assert!(
            cut > five,
            "cut {cut} must keep the ~5 s boundary (data platform / Snowflake / next checkpoint) intact"
        );
    }
}
