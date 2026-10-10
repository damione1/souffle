//! Shared batch-window cutting for Whisper and Parakeet.
//!
//! Both engines are stateless per window. Fixed 5 s non-overlapping cuts
//! split words at the boundary (`data | platform`, `Snow | flake`) and
//! push Parakeet into inventing a completion (`The next one is the same
//! thing.`). Cut on a silence gap in [4 s, 7 s] instead; if none, cut at
//! 7 s. A 100 ms delivery hop permits revisable snapshots without changing
//! these final cuts. Snapshots never consume or advance the buffered PCM.

use std::time::{Duration, Instant};

use super::{EngineError, TranscriptionSegment};

/// Whisper / Parakeet sample rate.
pub const SAMPLE_RATE: u32 = 16_000;

/// Audio delivery hop, independent of the final inference-window length.
pub const CHUNK_SAMPLES: usize = SAMPLE_RATE as usize / 10;

/// The previous 5 s delivery hop rounded both VAD holds up to 5 s. Keep
/// that audio retention budget when shortening delivery for previews.
pub const VAD_HOLD_SECONDS: f64 = 5.0;

const PREVIEW_INTERVAL: Duration = Duration::from_millis(1500);
const PREVIEW_SAMPLES: usize = SAMPLE_RATE as usize * 3 / 2;

/// Actor-owned, synchronous optional work. Audio and monotonic time both
/// advance before a revision; missed opportunities are coalesced, never queued.
pub(super) struct PreviewPolicy {
    enabled: bool,
    last_samples: usize,
    last_attempt: Option<Instant>,
    visible: bool,
}

impl Default for PreviewPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            last_samples: 0,
            last_attempt: None,
            visible: false,
        }
    }
}

impl PreviewPolicy {
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn reset(&mut self) {
        self.last_samples = 0;
        self.last_attempt = None;
        self.visible = false;
    }

    /// Withdraw a snapshot when its PCM is finalized, including a final
    /// window whose silence/hallucination filters produce no final words.
    pub fn clear(&mut self, offset: f64) -> Option<TranscriptionSegment> {
        std::mem::take(&mut self.visible).then(|| empty_preview(offset))
    }

    pub fn decode(
        &mut self,
        pcm: &[f32],
        consumed_samples: usize,
        now: Instant,
        decode: impl FnOnce(&[f32], f64) -> Result<Vec<TranscriptionSegment>, EngineError>,
    ) -> Option<TranscriptionSegment> {
        let total = consumed_samples + pcm.len();
        if !self.enabled
            || pcm.len() < PREVIEW_SAMPLES
            || total.saturating_sub(self.last_samples) < PREVIEW_SAMPLES
            || self
                .last_attempt
                .is_some_and(|last| now.saturating_duration_since(last) < PREVIEW_INTERVAL)
        {
            return None;
        }
        self.last_samples = total;
        self.last_attempt = Some(now);
        let offset = consumed_samples as f64 / SAMPLE_RATE as f64;
        // Silence must never be sent to Parakeet's decoder; Whisper also
        // applies its existing near-silence and hallucination gates.
        let result = if pcm.iter().all(|s| *s == 0.0) {
            Ok(Vec::new())
        } else {
            decode(pcm, offset)
        };
        match result {
            Ok(segments) => {
                let mut snapshot = empty_preview(offset);
                for segment in segments {
                    let text = segment.text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    if !snapshot.text.is_empty() {
                        snapshot.text.push(' ');
                    }
                    snapshot.text.push_str(text);
                    snapshot.end_time = snapshot.end_time.max(segment.end_time);
                    snapshot.language = snapshot.language.or(segment.language);
                }
                if snapshot.text.is_empty() {
                    self.clear(offset)
                } else {
                    self.visible = true;
                    Some(snapshot)
                }
            }
            Err(error) => {
                tracing::warn!(%error, "Optional batch preview failed; final PCM retained");
                None
            }
        }
    }
}

fn empty_preview(offset: f64) -> TranscriptionSegment {
    TranscriptionSegment {
        text: String::new(),
        start_time: offset,
        end_time: offset,
        is_final: false,
        speaker: None,
        language: None,
        confidence: None,
    }
}

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

    fn hypothesis(text: &str, start: f64, end: f64) -> TranscriptionSegment {
        TranscriptionSegment {
            text: text.into(),
            start_time: start,
            end_time: end,
            is_final: true,
            speaker: None,
            language: Some("fr".into()),
            confidence: None,
        }
    }

    #[test]
    fn first_preview_at_1500ms_and_revisions_need_new_pcm_and_monotonic_time() {
        let mut policy = PreviewPolicy::default();
        let clock = Instant::now();
        let mut pcm = Vec::new();
        let mut calls = 0;
        for hop in 1..=15 {
            pcm.extend(vec![0.1; CHUNK_SAMPLES]);
            let snapshot = policy.decode(
                &pcm,
                0,
                clock + Duration::from_millis(hop * 100),
                |_, offset| {
                    calls += 1;
                    Ok(vec![
                        hypothesis("Premier", offset, 1.5),
                        hypothesis("aperçu.", 1.0, 1.5),
                    ])
                },
            );
            if hop < 15 {
                assert!(snapshot.is_none());
            } else {
                let snapshot = snapshot.unwrap();
                assert_eq!(snapshot.text, "Premier aperçu.");
                assert!(!snapshot.is_final);
                assert_eq!(snapshot.start_time, 0.0);
            }
        }
        assert_eq!(calls, 1);
        let before = pcm.clone();
        assert!(
            policy
                .decode(&pcm, 0, clock + Duration::from_secs(10), |_, _| panic!(
                    "no new PCM"
                ))
                .is_none()
        );
        assert_eq!(pcm, before, "snapshot must not consume audio");
        pcm.extend(vec![0.1; PREVIEW_SAMPLES]);
        assert!(
            policy
                .decode(&pcm, 0, clock + Duration::from_millis(2999), |_, _| panic!(
                    "too soon"
                ))
                .is_none()
        );
        let revision = policy
            .decode(&pcm, 0, clock + Duration::from_secs(3), |_, offset| {
                Ok(vec![hypothesis("Révision", offset, 3.0)])
            })
            .unwrap();
        assert_eq!(revision.text, "Révision");
        assert!(!revision.is_final);
    }

    #[test]
    fn failed_preview_preserves_the_final_window_and_does_not_spin() {
        let mut policy = PreviewPolicy::default();
        let clock = Instant::now();
        let mut pcm = sine(1.5, 0.3);
        let before = pcm.clone();
        assert!(
            policy
                .decode(&pcm, 0, clock, |_, _| Err(EngineError::InferenceError(
                    "test failure".into()
                )))
                .is_none()
        );
        assert_eq!(pcm, before);
        assert!(
            policy
                .decode(&pcm, 0, clock + PREVIEW_INTERVAL, |_, _| panic!(
                    "no new audio after error"
                ))
                .is_none()
        );
        pcm.extend(sine(5.5, 0.3));
        let windows = drain_ready_windows(&mut pcm);
        assert_eq!(windows[0].len(), MAX_CUT_SAMPLES);
        assert_eq!(&windows[0][..before.len()], &before);
        assert!(pcm.is_empty());
    }

    #[test]
    fn silence_does_not_decode_and_empty_revisions_withdraw_visible_text() {
        let mut policy = PreviewPolicy::default();
        let clock = Instant::now();
        assert!(
            policy
                .decode(&silence(1.5), 0, clock, |_, _| panic!(
                    "digital silence must not decode"
                ))
                .is_none()
        );
        let offset_samples = SAMPLE_RATE as usize * 7;
        let snapshot = policy
            .decode(
                &sine(1.5, 0.3),
                offset_samples,
                clock + PREVIEW_INTERVAL,
                |_, offset| Ok(vec![hypothesis("Bonjour", offset, offset + 1.5)]),
            )
            .unwrap();
        assert_eq!(snapshot.start_time, 7.0);
        let withdrawn = policy
            .decode(
                &sine(3.0, 0.3),
                offset_samples,
                clock + PREVIEW_INTERVAL * 2,
                |_, _| Ok(vec![]),
            )
            .unwrap();
        assert!(!withdrawn.is_final);
        assert!(withdrawn.text.is_empty());
        assert!(policy.clear(7.0).is_none(), "withdraw only once");
    }

    #[test]
    fn catch_up_coalesces_previews_and_reset_starts_a_fresh_session() {
        let mut policy = PreviewPolicy::default();
        let clock = Instant::now();
        policy.set_enabled(false);
        for hop in 15..=60 {
            let pcm = sine(hop as f64 / 10.0, 0.3);
            assert!(
                policy
                    .decode(&pcm, 0, clock, |_, _| panic!("catch-up preview"))
                    .is_none()
            );
        }
        policy.set_enabled(true);
        let snapshot = policy
            .decode(&sine(6.0, 0.3), 0, clock, |_, offset| {
                Ok(vec![hypothesis("Un seul aperçu", offset, 6.0)])
            })
            .unwrap();
        assert_eq!(snapshot.text, "Un seul aperçu");
        policy.reset();
        assert!(policy.clear(0.0).is_none());
        assert!(
            policy
                .decode(&sine(1.5, 0.3), 0, clock, |_, offset| Ok(vec![hypothesis(
                    "Nouvelle session",
                    offset,
                    1.5
                )]))
                .is_some()
        );
    }

    #[test]
    fn previews_preserve_final_pcm_and_offsets_across_cuts_and_short_tail() {
        let mut source = sine(6.0, 0.3);
        source.extend(silence(0.3));
        source.extend(sine(7.1, 0.3)); // forced cut, then short tail
        let run = |previews: bool| {
            let mut policy = PreviewPolicy::default();
            policy.set_enabled(previews);
            let mut buffer = Vec::new();
            let mut finals = Vec::new();
            let mut consumed = 0;
            let clock = Instant::now();
            for (i, hop) in source.chunks(CHUNK_SAMPLES).enumerate() {
                buffer.extend_from_slice(hop);
                for window in drain_ready_windows(&mut buffer) {
                    policy.clear(consumed as f64 / SAMPLE_RATE as f64);
                    let len = window.len();
                    finals.push((consumed, window));
                    consumed += len;
                }
                policy.decode(
                    &buffer,
                    consumed,
                    clock + Duration::from_millis(i as u64 * 100),
                    |_, offset| Ok(vec![hypothesis("Révisable", offset, offset + 1.5)]),
                );
            }
            finals.push((consumed, buffer));
            finals
        };
        let baseline = run(false);
        assert_eq!(run(true), baseline);
        assert_eq!(baseline[0].0, 0);
        assert_eq!(baseline[0].1.len(), SAMPLE_RATE as usize * 6);
        assert_eq!(baseline[1].1.len(), MAX_CUT_SAMPLES);
        assert!(baseline.last().unwrap().1.len() < SAMPLE_RATE as usize / 2);
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
        let five = SAMPLE_RATE as usize * 5;
        assert!(
            cut > five,
            "cut {cut} must keep the ~5 s boundary (data platform / Snowflake / next checkpoint) intact"
        );
    }
}
