//! Source-separated batch state; both lanes borrow one loaded ASR model.
use super::batch_windows::{PreviewPolicy, SAMPLE_RATE, drain_ready_windows};
use super::{EngineError, Speaker, TranscriptionSegment};
use crate::filter::{
    AudioFilterChain, PipelineConfig, build_audio_filters, resolve_vad_model_path,
};
use std::time::Instant;

#[derive(Default)]
pub(super) struct Lane {
    pcm: Vec<f32>,
    consumed: usize,
    language: Option<String>,
    preview: PreviewPolicy,
    speech_gate: Option<AudioFilterChain>,
}
impl Lane {
    fn dual() -> Self {
        // Separate Silero recurrent state per source. Snapshot classification
        // resets it so previews cannot influence a final or the other source.
        let speech_gate = resolve_vad_model_path().map(|path| {
            build_audio_filters(
                &PipelineConfig {
                    vad_enabled: true,
                    vad_model_path: Some(path),
                    filler_removal_enabled: false,
                    stutter_collapse_enabled: false,
                    dictionary_correction_enabled: false,
                },
                SAMPLE_RATE,
                None,
            )
        });
        Self {
            speech_gate,
            ..Self::default()
        }
    }
    fn reset(&mut self, preserve: bool) {
        self.pcm.clear();
        self.preview.reset();
        if !preserve {
            self.consumed = 0;
            self.language = None;
        }
        if let Some(gate) = &mut self.speech_gate {
            gate.reset();
        }
    }
    fn speech(gate: &mut Option<AudioFilterChain>, pcm: &[f32]) -> bool {
        if pcm.iter().all(|s| *s == 0.0) {
            return false;
        }
        match gate {
            Some(gate) => {
                gate.reset();
                gate.process(pcm)
            }
            None => true,
        }
    }
}
pub(super) enum BatchSession {
    Mono(Lane),
    Dual {
        me: Lane,
        them: Lane,
        next_preview: Speaker,
    },
}
impl Default for BatchSession {
    fn default() -> Self {
        Self::Mono(Lane::default())
    }
}
type DecodeResult = Result<(Vec<TranscriptionSegment>, Option<String>), EngineError>;

/// Decoder times are local to this captured window, regardless of the
/// model's internal frame size or the inference padding on a short tail.
/// Normalize before adding the shared session offset. Valid times retain
/// their precision; malformed times cannot poison chronological grouping.
fn normalize_window_timestamps(segments: &mut [TranscriptionSegment], samples: usize) {
    let duration = samples as f64 / SAMPLE_RATE as f64;
    for segment in segments {
        let start = if segment.start_time.is_finite() {
            segment.start_time.clamp(0.0, duration)
        } else {
            0.0
        };
        let end = if segment.end_time.is_finite() {
            segment.end_time.clamp(start, duration)
        } else {
            start
        };
        segment.start_time = start;
        segment.end_time = end;
    }
}
/// Cache auto-detected language only from a window that kept real speech.
/// A filtered leading hallucination must not lock the session to the
/// wrong language for later windows.
pub(super) fn remember_detected_language(
    cached: &mut Option<String>,
    requested: Option<&str>,
    detected: Option<String>,
    surviving_segments: &[TranscriptionSegment],
) {
    if requested.is_some() || cached.is_some() || surviving_segments.is_empty() {
        return;
    }
    if let Some(lang) = detected {
        tracing::info!(language = %lang, "Whisper auto-detected language, caching for session");
        *cached = Some(lang);
    }
}

impl BatchSession {
    #[cfg(any(test, feature = "test-support"))]
    pub fn controlled_dual() -> Self {
        Self::Dual {
            me: Lane::default(),
            them: Lane::default(),
            next_preview: Speaker::Me,
        }
    }
    pub fn set_dual(&mut self, dual: bool) {
        match (&self, dual) {
            (Self::Mono(_), false) | (Self::Dual { .. }, true) => {}
            (Self::Mono(_), true) => {
                *self = Self::Dual {
                    me: Lane::dual(),
                    them: Lane::dual(),
                    next_preview: Speaker::Me,
                }
            }
            (Self::Dual { .. }, false) => *self = Self::default(),
        }
    }
    pub fn reset(&mut self, preserve: bool) {
        match self {
            Self::Mono(lane) => lane.reset(preserve),
            Self::Dual {
                me,
                them,
                next_preview,
            } => {
                if preserve {
                    // Recovery discards a wedged window but must not rewind
                    // either source's capture clock by its buffered duration.
                    me.consumed += me.pcm.len();
                    them.consumed += them.pcm.len();
                }
                me.reset(preserve);
                them.reset(preserve);
                *next_preview = Speaker::Me;
            }
        }
    }
    pub fn set_preview_enabled(&mut self, enabled: bool) {
        match self {
            Self::Mono(lane) => lane.preview.set_enabled(enabled),
            Self::Dual { me, them, .. } => {
                me.preview.set_enabled(enabled);
                them.preview.set_enabled(enabled);
            }
        }
    }
    pub fn transcribe(
        &mut self,
        pcm: &[f32],
        language: Option<&str>,
        mut decode: impl FnMut(&[f32], &[f32], Option<&str>) -> DecodeResult,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let Self::Mono(lane) = self else {
            return Err(EngineError::InferenceError(
                "Mono audio supplied to dual batch session".into(),
            ));
        };
        lane.pcm.extend_from_slice(pcm);
        let (mut segments, finalized) = Self::finals(lane, None, language, &mut decode)?;
        if !finalized {
            segments.extend(Self::preview(
                lane,
                None,
                language,
                Instant::now(),
                &mut decode,
            ));
        }
        Ok(segments)
    }
    pub fn dual(
        &mut self,
        mic: &[f32],
        system: &[f32],
        decode: impl FnMut(&[f32], &[f32], Option<&str>) -> DecodeResult,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        self.dual_at(mic, system, Instant::now(), decode)
    }
    pub(super) fn dual_at(
        &mut self,
        mic: &[f32],
        system: &[f32],
        now: Instant,
        mut decode: impl FnMut(&[f32], &[f32], Option<&str>) -> DecodeResult,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let Self::Dual {
            me,
            them,
            next_preview,
        } = self
        else {
            return Err(EngineError::InferenceError(
                "Dual audio supplied to mono batch session".into(),
            ));
        };
        let len = mic.len().max(system.len());
        me.pcm.extend_from_slice(mic);
        me.pcm.resize(me.pcm.len() + len - mic.len(), 0.0);
        them.pcm.extend_from_slice(system);
        them.pcm.resize(them.pcm.len() + len - system.len(), 0.0);
        // Finals on BOTH sources precede optional work. One snapshot per
        // actor turn, alternating opportunities rather than queued jobs.
        let (mut segments, me_final) = Self::finals(me, Some(Speaker::Me), None, &mut decode)?;
        let (other, them_final) = Self::finals(them, Some(Speaker::Them), None, &mut decode)?;
        segments.extend(other);
        if !me_final && !them_final {
            match next_preview {
                Speaker::Me => {
                    segments.extend(Self::preview(me, Some(Speaker::Me), None, now, &mut decode));
                    *next_preview = Speaker::Them;
                }
                Speaker::Them => {
                    segments.extend(Self::preview(
                        them,
                        Some(Speaker::Them),
                        None,
                        now,
                        &mut decode,
                    ));
                    *next_preview = Speaker::Me;
                }
            }
        }
        segments.sort_by(|a, b| a.start_time.total_cmp(&b.start_time));
        Ok(segments)
    }
    fn finals(
        lane: &mut Lane,
        speaker: Option<Speaker>,
        language: Option<&str>,
        decode: &mut impl FnMut(&[f32], &[f32], Option<&str>) -> DecodeResult,
    ) -> Result<(Vec<TranscriptionSegment>, bool), EngineError> {
        let windows = drain_ready_windows(&mut lane.pcm);
        let finalized = !windows.is_empty();
        let mut out = Vec::new();
        if finalized {
            out.extend(
                lane.preview
                    .clear(lane.consumed as f64 / SAMPLE_RATE as f64)
                    .map(|mut s| {
                        s.speaker = speaker;
                        s
                    }),
            );
        }
        for pcm in windows {
            out.extend(Self::finish_window(
                lane, speaker, language, &pcm, &pcm, decode,
            )?);
        }
        Ok((out, finalized))
    }
    fn finish_window(
        lane: &mut Lane,
        speaker: Option<Speaker>,
        language: Option<&str>,
        pcm: &[f32],
        original: &[f32],
        decode: &mut impl FnMut(&[f32], &[f32], Option<&str>) -> DecodeResult,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let offset = lane.consumed as f64 / SAMPLE_RATE as f64;
        lane.consumed += original.len();
        if !Lane::speech(&mut lane.speech_gate, original) {
            return Ok(vec![]);
        }
        let (mut segments, detected) =
            decode(pcm, original, language.or(lane.language.as_deref()))?;
        normalize_window_timestamps(&mut segments, original.len());
        remember_detected_language(&mut lane.language, language, detected, &segments);
        for s in &mut segments {
            s.start_time += offset;
            s.end_time += offset;
            s.speaker = speaker;
        }
        Ok(segments)
    }
    fn preview(
        lane: &mut Lane,
        speaker: Option<Speaker>,
        language: Option<&str>,
        now: Instant,
        decode: &mut impl FnMut(&[f32], &[f32], Option<&str>) -> DecodeResult,
    ) -> Option<TranscriptionSegment> {
        let lang = language.or(lane.language.as_deref());
        let gate = &mut lane.speech_gate;
        lane.preview
            .decode(&lane.pcm, lane.consumed, now, |pcm, offset| {
                if !Lane::speech(gate, pcm) {
                    return Ok(vec![]);
                }
                let (mut segments, _) = decode(pcm, pcm, lang)?;
                normalize_window_timestamps(&mut segments, pcm.len());
                for s in &mut segments {
                    s.start_time += offset;
                    s.end_time += offset;
                }
                Ok(segments)
            })
            .map(|mut s| {
                s.speaker = speaker;
                s
            })
    }
    pub fn flush(
        &mut self,
        minimum: usize,
        mut decode: impl FnMut(&[f32], &[f32], Option<&str>) -> DecodeResult,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        fn lane(
            lane: &mut Lane,
            speaker: Option<Speaker>,
            minimum: usize,
            decode: &mut impl FnMut(&[f32], &[f32], Option<&str>) -> DecodeResult,
        ) -> Result<Vec<TranscriptionSegment>, EngineError> {
            if lane.pcm.is_empty() {
                return Ok(vec![]);
            }
            let mut pcm = std::mem::take(&mut lane.pcm);
            let len = pcm.len();
            pcm.resize(len.max(minimum), 0.0);
            let mut out: Vec<_> = lane
                .preview
                .clear(lane.consumed as f64 / SAMPLE_RATE as f64)
                .map(|mut s| {
                    s.speaker = speaker;
                    s
                })
                .into_iter()
                .collect();
            out.extend(BatchSession::finish_window(
                lane,
                speaker,
                None,
                &pcm,
                &pcm[..len],
                decode,
            )?);
            Ok(out)
        }
        let mut out = match self {
            Self::Mono(state) => lane(state, None, minimum, &mut decode)?,
            Self::Dual { me, them, .. } => {
                let mut out = lane(me, Some(Speaker::Me), minimum, &mut decode)?;
                out.extend(lane(them, Some(Speaker::Them), minimum, &mut decode)?);
                out
            }
        };
        out.sort_by(|a, b| a.start_time.total_cmp(&b.start_time));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn timed_segment(start: f64, end: f64) -> TranscriptionSegment {
        TranscriptionSegment {
            text: "Recognized speech".into(),
            start_time: start,
            end_time: end,
            is_final: true,
            speaker: None,
            language: Some("fr".into()),
            confidence: Some(0.9),
        }
    }

    #[test]
    fn decoded_window_bounds_keep_valid_times_and_repair_invalid_times() {
        let mut segments = vec![
            timed_segment(1.75, 4.2),
            timed_segment(0.0, 30.0),
            timed_segment(-0.5, 30.0),
            timed_segment(3.0, 1.5),
            timed_segment(6.0, 30.0),
            timed_segment(f64::NAN, f64::INFINITY),
        ];
        normalize_window_timestamps(&mut segments, 5 * SAMPLE_RATE as usize);
        let bounds: Vec<_> = segments
            .iter()
            .map(|s| (s.start_time, s.end_time))
            .collect();
        assert_eq!(
            bounds,
            vec![
                (1.75, 4.2),
                (0.0, 5.0),
                (0.0, 5.0),
                (3.0, 3.0),
                (5.0, 5.0),
                (0.0, 0.0)
            ]
        );
        assert!(segments.iter().all(|s| s.text == "Recognized speech"
            && s.is_final
            && s.language.as_deref() == Some("fr")
            && s.confidence == Some(0.9)));
    }

    #[test]
    fn decoded_final_bounds_use_original_tail_before_session_offset() {
        let mut lane = Lane {
            consumed: 8 * SAMPLE_RATE as usize,
            ..Lane::default()
        };
        let original = [0.1; 3200];
        let padded = [0.1; 16000];
        let mut decoder =
            |_: &[f32], _: &[f32], _: Option<&str>| Ok((vec![timed_segment(0.05, 30.0)], None));
        let finals = BatchSession::finish_window(
            &mut lane,
            Some(Speaker::Them),
            None,
            &padded,
            &original,
            &mut decoder,
        )
        .unwrap();
        assert_eq!(finals[0].start_time, 8.05);
        assert_eq!(finals[0].end_time, 8.2);
        assert_eq!(finals[0].speaker, Some(Speaker::Them));
        assert_eq!(lane.consumed, 8 * SAMPLE_RATE as usize + original.len());
    }

    #[test]
    fn decoded_preview_and_final_share_bounds_without_consuming_preview_pcm() {
        let mut lane = Lane {
            consumed: 8 * SAMPLE_RATE as usize,
            pcm: vec![0.1; 2 * SAMPLE_RATE as usize],
            ..Lane::default()
        };
        let mut decoder =
            |_: &[f32], _: &[f32], _: Option<&str>| Ok((vec![timed_segment(0.25, 30.0)], None));
        let preview = BatchSession::preview(
            &mut lane,
            Some(Speaker::Me),
            None,
            Instant::now(),
            &mut decoder,
        )
        .unwrap();
        assert!(!preview.is_final);
        assert_eq!((preview.start_time, preview.end_time), (8.0, 10.0));
        assert_eq!(preview.speaker, Some(Speaker::Me));
        assert_eq!(lane.consumed, 8 * SAMPLE_RATE as usize);
        assert_eq!(lane.pcm.len(), 2 * SAMPLE_RATE as usize);
        let pcm = lane.pcm.clone();
        let finals = BatchSession::finish_window(
            &mut lane,
            Some(Speaker::Me),
            None,
            &pcm,
            &pcm,
            &mut decoder,
        )
        .unwrap();
        assert_eq!((finals[0].start_time, finals[0].end_time), (8.25, 10.0));
        assert_eq!(lane.consumed, 10 * SAMPLE_RATE as usize);
    }

    #[test]
    fn dual_recovery_preserves_both_capture_clocks_and_cached_languages() {
        let mut session = BatchSession::controlled_dual();
        session.set_preview_enabled(false);
        session
            .dual(&[0.1; 16000 * 8], &[0.2; 16000 * 8], decode)
            .unwrap();
        session.reset(true);
        let mut requested = Vec::new();
        session.dual(&[0.1; 3200], &[0.2; 3200], decode).unwrap();
        let out = session
            .flush(8000, |a, b, l| {
                requested.push(l.map(str::to_owned));
                decode(a, b, l)
            })
            .unwrap();
        assert_eq!(requested, vec![Some("Me".into()), Some("Them".into())]);
        assert!(out.iter().all(|s| s.start_time == 8.0));
        session.reset(false);
        session.dual(&[0.1; 3200], &[0.2; 3200], decode).unwrap();
        let mut requested = Vec::new();
        let out = session
            .flush(8000, |a, b, l| {
                requested.push(l.map(str::to_owned));
                decode(a, b, l)
            })
            .unwrap();
        assert!(requested.iter().all(Option::is_none));
        assert!(out.iter().all(|s| s.start_time == 0.0));
    }

    fn decode(pcm: &[f32], _: &[f32], language: Option<&str>) -> DecodeResult {
        let start = pcm.iter().position(|s| *s != 0.0).unwrap_or(0);
        let name = if pcm[start] > 0.15 { "Them" } else { "Me" };
        Ok((
            vec![TranscriptionSegment {
                text: name.into(),
                start_time: start as f64 / SAMPLE_RATE as f64,
                end_time: pcm.len() as f64 / SAMPLE_RATE as f64,
                is_final: true,
                speaker: None,
                language: language.map(str::to_owned),
                confidence: None,
            }],
            Some(name.into()),
        ))
    }

    #[test]
    fn two_sources_preview_before_finals_keep_independent_language_and_pcm() {
        let mut session = BatchSession::controlled_dual();
        let now = Instant::now();
        let mut previews = Vec::new();
        for i in 1..=16 {
            previews.extend(
                session
                    .dual_at(
                        &[0.1; 1600],
                        &[0.2; 1600],
                        now + Duration::from_millis(i * 100),
                        decode,
                    )
                    .unwrap(),
            );
        }
        assert_eq!(previews.len(), 2);
        assert_eq!(previews[0].speaker, Some(Speaker::Me));
        assert_eq!(previews[1].speaker, Some(Speaker::Them));
        assert!(previews.iter().all(|s| !s.is_final));
        let BatchSession::Dual { me, them, .. } = &session else {
            unreachable!()
        };
        assert!(
            me.language.is_none() && them.language.is_none(),
            "preview language cached"
        );
        assert_eq!(me.pcm.len(), 25600);
        assert_eq!(them.pcm.len(), 25600);
        let flushed = session.flush(8000, decode).unwrap();
        let withdrawals: Vec<_> = flushed
            .iter()
            .filter(|s| !s.is_final)
            .map(|s| s.speaker)
            .collect();
        assert_eq!(withdrawals, vec![Some(Speaker::Me), Some(Speaker::Them)]);
        let finals: Vec<_> = flushed.into_iter().filter(|s| s.is_final).collect();
        assert_eq!(finals.len(), 2);
        assert!(
            finals
                .iter()
                .all(|s| s.start_time == 0.0 && s.end_time == 1.6)
        );
        let BatchSession::Dual { me, them, .. } = &session else {
            unreachable!()
        };
        assert_eq!(me.language.as_deref(), Some("Me"));
        assert_eq!(them.language.as_deref(), Some("Them"));
        assert!(session.flush(8000, decode).unwrap().is_empty());
    }

    #[test]
    fn asymmetric_chunks_silent_lane_and_short_tail_share_one_clock() {
        let mut session = BatchSession::controlled_dual();
        session.set_preview_enabled(false);
        let first = session.dual(&[0.1; 112000], &[], decode).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].speaker, Some(Speaker::Me));
        let second = session.dual(&[], &[0.2; 3200], decode).unwrap();
        assert!(second.is_empty());
        let final_tail = session.flush(8000, decode).unwrap();
        assert_eq!(final_tail.len(), 1);
        assert_eq!(final_tail[0].speaker, Some(Speaker::Them));
        assert_eq!(final_tail[0].start_time, 7.0);
        assert!(session.flush(8000, decode).unwrap().is_empty());
        session.set_dual(false);
        session.reset(false);
        session.transcribe(&[0.1; 3200], None, decode).unwrap();
        assert!(
            session
                .flush(8000, decode)
                .unwrap()
                .iter()
                .all(|s| s.speaker.is_none() && s.start_time == 0.0)
        );
    }

    #[test]
    fn previews_do_not_change_two_lane_final_windows_or_stop_remainders() {
        let mut baseline = None;
        for enabled in [false, true] {
            let mut session = BatchSession::controlled_dual();
            session.set_preview_enabled(enabled);
            let now = Instant::now();
            let mut finals = Vec::new();
            let mut pcm_sizes = Vec::new();
            for i in 1..=143 {
                let pcm = if (40..46).contains(&i) {
                    [0.0; 1600]
                } else {
                    [0.1; 1600]
                };
                let segments = session
                    .dual_at(
                        &pcm,
                        &[0.2; 1600],
                        now + Duration::from_millis(i * 100),
                        |a, b, l| {
                            pcm_sizes.push(b.len());
                            decode(a, b, l)
                        },
                    )
                    .unwrap();
                finals.extend(segments.into_iter().filter(|s| s.is_final));
            }
            finals.extend(
                session
                    .flush(16000, decode)
                    .unwrap()
                    .into_iter()
                    .filter(|s| s.is_final),
            );
            let snapshots: Vec<_> = finals
                .iter()
                .map(|s| (s.text.clone(), s.speaker, s.start_time, s.end_time))
                .collect();
            if let Some(before) = &baseline {
                assert_eq!(before, &snapshots);
            } else {
                baseline = Some(snapshots);
            }
        }
    }
}
