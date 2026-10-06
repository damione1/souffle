use std::path::{Path, PathBuf};

use tracing::{debug, info};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use super::batch_session::BatchSession;
use super::batch_windows::{CHUNK_SAMPLES, SAMPLE_RATE as WHISPER_SAMPLE_RATE, pcm_rms};
use super::{
    AudioInputRequirements, EngineError, TranscriptionEngine, TranscriptionSegment,
    collapse_whitespace,
};

/// Number of CPU threads for whisper.cpp inference.
const WHISPER_N_THREADS: i32 = 4;

/// Minimum audio for meaningful inference (1 second).
const MIN_INFERENCE_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize;

/// Segments with no-speech probability above this are discarded.
const NO_SPEECH_PROB_THRESHOLD: f32 = 0.6;

/// Digital-silence / near-zero RMS. Room tone sits well above this; those
/// windows are dropped via [`is_known_hallucination`] instead.
const SILENCE_RMS_FLOOR: f32 = 5e-4;

/// Strip whisper special tokens: [_BEG_], [_TT_xxx], [_SOT_], [_EOT_], [_LANG_xx], etc.
fn strip_special_tokens(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(&ch) = chars.peek() {
        if ch == '[' {
            let mut token = String::new();
            token.push(ch);
            chars.next();

            let mut is_special = false;
            while let Some(&c) = chars.peek() {
                token.push(c);
                chars.next();
                if c == ']' {
                    if token.starts_with("[_") {
                        is_special = true;
                    }
                    break;
                }
            }

            if !is_special {
                result.push_str(&token);
            }
        } else {
            result.push(ch);
            chars.next();
        }
    }

    collapse_whitespace(&result)
}

/// Alphanumeric-only lowercase key so `"Thank you."` / `". . Thank you."`
/// collapse to the same token.
fn hallucination_key(text: &str) -> String {
    let lowered = text.to_lowercase();
    let spaced: String = lowered
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    collapse_whitespace(&spaced)
}

/// True when `text` is *only* a known Whisper hallucination, not real
/// speech that happens to include "thank you" or mention Amara.
fn is_known_hallucination(text: &str) -> bool {
    let key = hallucination_key(text);
    if key.is_empty() {
        return false;
    }
    matches!(
        key.as_str(),
        "thank you" | "thank you thank you" | "merci" | "amara" | "amara org"
    ) || is_subtitle_credit(&key)
}

/// Whole-utterance subtitle watermarks. Prefixes, not substrings, so
/// "I spoke to Amara yesterday" / "the subtitles are ready" survive.
fn is_subtitle_credit(key: &str) -> bool {
    const CREDIT_PREFIXES: &[&str] = &[
        "sous titres réalisés par",
        "sous titres par amara",
        "subtitles by",
        "subtitles made",
    ];
    CREDIT_PREFIXES.iter().any(|prefix| key.starts_with(prefix))
}

/// Drop a window that is digital silence, or whose sole segment is a
/// known hallucination. Real speech that mentions "thank you" is kept.
fn drop_silent_or_hallucinated_window(
    segments: Vec<TranscriptionSegment>,
    audio: &[f32],
) -> Vec<TranscriptionSegment> {
    if pcm_rms(audio) < SILENCE_RMS_FLOOR {
        return Vec::new();
    }
    if segments.len() == 1 && is_known_hallucination(&segments[0].text) {
        return Vec::new();
    }
    segments
}

struct LoadedWhisperModel {
    ctx: WhisperContext,
    #[allow(dead_code)]
    model_path: PathBuf,
}

/// Whisper STT engine via whisper-rs (whisper.cpp bindings).
/// Batch-oriented: accumulates audio, cuts on a silence gap in [4 s, 7 s]
/// (else 7 s), with revisable snapshots of the remainder every 1.5 s.
/// This is batch re-inference, not native streaming decoding.
pub struct WhisperEngine {
    model: Option<LoadedWhisperModel>,
    session: BatchSession,
}

impl Default for WhisperEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl WhisperEngine {
    pub fn new() -> Self {
        Self {
            model: None,
            session: BatchSession::default(),
        }
    }

    /// Run inference on a chunk of audio. Returns detected language code
    /// alongside the transcription segments.
    fn run_inference(
        ctx: &WhisperContext,
        audio: &[f32],
        rms_audio: &[f32],
        language: Option<&str>,
    ) -> Result<(Vec<TranscriptionSegment>, Option<String>), EngineError> {
        if audio.is_empty() || pcm_rms(rms_audio) < SILENCE_RMS_FLOOR {
            return Ok((vec![], None));
        }

        let mut state = ctx
            .create_state()
            .map_err(|e| EngineError::InferenceError(format!("Whisper state creation: {e}")))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(WHISPER_N_THREADS);
        params.set_no_context(true);
        params.set_single_segment(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        // IMPORTANT: do NOT use set_detect_language(true) — in whisper.cpp that
        // flag means "detect language and return WITHOUT transcribing" (early return 0).
        // For auto-detection that also transcribes, set language to None instead.
        if let Some(lang) = language {
            params.set_language(Some(lang));
        } else {
            params.set_language(None);
        }

        state
            .full(params, audio)
            .map_err(|e| EngineError::InferenceError(format!("Whisper inference: {e}")))?;

        // Extract detected language from state (works for both auto-detect and explicit)
        let detected_lang = {
            let lang_id = state.full_lang_id_from_state();
            whisper_rs::get_lang_str(lang_id).map(String::from)
        };

        let n_segments = state.full_n_segments();
        if crate::debug::transcription_debug_enabled() {
            debug!(
                n_segments,
                audio_samples = audio.len(),
                language = ?language,
                detected = ?detected_lang,
                "Whisper inference complete"
            );
        }

        let mut segments = Vec::new();
        for i in 0..n_segments {
            let Some(seg) = state.get_segment(i) else {
                continue;
            };

            let no_speech = seg.no_speech_probability();
            if no_speech > NO_SPEECH_PROB_THRESHOLD {
                continue;
            }

            let text = match seg.to_str() {
                Ok(t) => t.to_string(),
                Err(_) => match seg.to_str_lossy() {
                    Ok(t) => t.to_string(),
                    Err(_) => continue,
                },
            };

            if text.trim().is_empty() {
                continue;
            }

            segments.push(TranscriptionSegment {
                text,
                start_time: seg.start_timestamp() as f64 / 100.0,
                end_time: seg.end_timestamp() as f64 / 100.0,
                is_final: true,
                language: detected_lang.clone().or_else(|| language.map(String::from)),
                confidence: Some(1.0 - no_speech),
                speaker: None,
            });
        }

        segments = drop_silent_or_hallucinated_window(segments, rms_audio);

        Ok((segments, detected_lang))
    }
}

impl TranscriptionEngine for WhisperEngine {
    fn load_model(&mut self, model_path: &Path) -> Result<(), EngineError> {
        let bin_path = if model_path.extension().is_some_and(|ext| ext == "bin") {
            model_path.to_path_buf()
        } else {
            let entries = std::fs::read_dir(model_path)
                .map_err(|e| EngineError::ModelNotFound(model_path.join(format!("*.bin ({e})"))))?;
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|ext| ext == "bin"))
                .ok_or_else(|| EngineError::ModelNotFound(model_path.join("*.bin")))?
        };

        if !bin_path.exists() {
            return Err(EngineError::ModelNotFound(bin_path));
        }

        info!(path = %bin_path.display(), "Loading Whisper model");

        let ctx = WhisperContext::new_with_params(
            bin_path
                .to_str()
                .ok_or_else(|| EngineError::LoadError("Invalid model path encoding".into()))?,
            WhisperContextParameters::default(),
        )
        .map_err(|e| EngineError::LoadError(format!("Whisper model load: {e}")))?;

        self.model = Some(LoadedWhisperModel {
            ctx,
            model_path: bin_path,
        });

        info!("Whisper model loaded");
        Ok(())
    }

    fn unload_model(&mut self) -> Result<(), EngineError> {
        self.model = None;
        self.session.reset(false);

        info!("Whisper model unloaded");
        Ok(())
    }

    fn transcribe(
        &mut self,
        audio: &[f32],
        language: Option<&str>,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let loaded = self.model.as_ref().ok_or(EngineError::NotInitialized)?;
        self.session
            .transcribe(audio, language, |pcm, original, language| {
                Self::run_inference(&loaded.ctx, pcm, original, language)
            })
    }

    fn transcribe_dual(
        &mut self,
        mic: &[f32],
        system: &[f32],
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let loaded = self.model.as_ref().ok_or(EngineError::NotInitialized)?;
        self.session.dual(mic, system, |pcm, original, language| {
            Self::run_inference(&loaded.ctx, pcm, original, language)
        })
    }

    fn flush(&mut self) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let loaded = self.model.as_ref().ok_or(EngineError::NotInitialized)?;
        self.session
            .flush(MIN_INFERENCE_SAMPLES, |pcm, original, language| {
                Self::run_inference(&loaded.ctx, pcm, original, language)
            })
    }

    fn supports_diarization(&self) -> bool {
        true
    }
    fn set_diarization(&mut self, enabled: bool) {
        self.session.set_dual(enabled);
    }
    fn reset_state(&mut self) -> Result<(), EngineError> {
        self.session.reset(false);
        Ok(())
    }
    fn reset_state_preserving_timeline(&mut self) -> Result<(), EngineError> {
        self.session.reset(true);
        Ok(())
    }

    fn audio_requirements(&self) -> AudioInputRequirements {
        AudioInputRequirements {
            sample_rate_hz: WHISPER_SAMPLE_RATE,
            channels: 1,
            chunk_size_samples: CHUNK_SAMPLES as u32,
        }
    }

    fn set_preview_enabled(&mut self, enabled: bool) {
        self.session.set_preview_enabled(enabled);
    }

    fn minimum_vad_hold_seconds(&self) -> f64 {
        super::batch_windows::VAD_HOLD_SECONDS
    }

    fn normalize_text(&self, text: &str) -> String {
        strip_special_tokens(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::batch_session::remember_detected_language;

    #[test]
    #[ignore = "requires catalogue Whisper weights; real timestamps across a pause"]
    fn weight_backed_whisper_timestamps_respect_captured_audio() {
        whisper_rs::install_logging_hooks();
        let profile =
            crate::engine::resolve_transcription_profile(Some("whisper"), None, None).unwrap();
        let mut engine = WhisperEngine::new();
        engine
            .load_model(&crate::models::model_dir(&profile))
            .unwrap();
        engine.set_preview_enabled(false);
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/audio/sou-030/hesitation-a-gap0400ms.wav");
        let mut reader = hound::WavReader::open(path).unwrap();
        let rate = reader.spec().sample_rate;
        assert_eq!(reader.spec().channels, 1);
        let source: Vec<f32> = reader
            .samples::<i16>()
            .map(|sample| sample.unwrap() as f32 / 32768.0)
            .collect();
        let mut resampler = crate::audio::Resampler::new(rate, 1, WHISPER_SAMPLE_RATE, 1.0);
        let mut speech = resampler.process(&source);
        speech.extend(resampler.flush());
        let speech: Vec<f32> = speech.into_iter().cycle().take(5 * 16000).collect();
        let mut pcm = speech.clone();
        pcm.resize(8 * 16000, 0.0);
        pcm.extend(speech);
        let mut finals = Vec::new();
        for (index, chunk) in pcm.chunks(1600).enumerate() {
            let captured = ((index + 1) * 1600) as f64 / WHISPER_SAMPLE_RATE as f64;
            for segment in engine.transcribe(chunk, None).unwrap() {
                if segment.is_final {
                    assert!(
                        segment.end_time <= captured,
                        "{segment:?} exceeds {captured}s capture"
                    );
                    finals.push(segment);
                }
            }
        }
        finals.extend(engine.flush().unwrap().into_iter().filter(|s| s.is_final));
        assert!(!finals.is_empty());
        assert!(finals.iter().all(|s| s.start_time >= 0.0
            && s.start_time <= s.end_time
            && s.end_time <= pcm.len() as f64 / WHISPER_SAMPLE_RATE as f64));
        assert!(
            finals.iter().any(|s| s.start_time >= 8.0),
            "post-pause speech has no later timestamp: {finals:?}"
        );
        assert!(engine.flush().unwrap().is_empty());
    }

    #[test]
    fn strip_special_tokens_removes_timing_tokens() {
        let input = "[_BEG_] Le cuisinier secoue les nouilles.[_TT_150]";
        assert_eq!(
            strip_special_tokens(input),
            "Le cuisinier secoue les nouilles."
        );
    }

    #[test]
    fn strip_special_tokens_removes_multiple_tokens() {
        let input = "[_TT_50] Hello[_TT_130][_TT_139] world.[_TT_259]";
        assert_eq!(strip_special_tokens(input), "Hello world.");
    }

    #[test]
    fn strip_special_tokens_preserves_normal_brackets() {
        let input = "He said [hello] to everyone.";
        assert_eq!(strip_special_tokens(input), "He said [hello] to everyone.");
    }

    #[test]
    fn strip_special_tokens_empty_after_strip() {
        let input = "[_BEG_][_TT_100]";
        assert_eq!(strip_special_tokens(input), "");
    }

    #[test]
    fn strip_special_tokens_cleans_extra_spaces() {
        let input = "[_BEG_]  Hello  [_TT_100]  world  [_TT_200]";
        assert_eq!(strip_special_tokens(input), "Hello world");
    }

    #[test]
    fn known_hallucinations_are_dropped() {
        for text in [
            "Thank you.",
            "Thank you. Thank you.",
            ". . Thank you.",
            "Merci.",
            "Sous-titres réalisés par Amara.org",
        ] {
            assert!(
                is_known_hallucination(text),
                "expected hallucination: {text:?}"
            );
        }
    }

    #[test]
    fn real_speech_with_thank_you_is_kept() {
        assert!(!is_known_hallucination("Thank you for the update."));
        assert!(!is_known_hallucination("Please thank you later today"));
        assert!(!is_known_hallucination("Merci beaucoup à toute l'équipe"));
        assert!(!is_known_hallucination("I spoke to Amara yesterday"));
        assert!(!is_known_hallucination("The subtitles are ready"));
    }

    #[test]
    fn filtered_leading_window_does_not_cache_language() {
        let mut cached = None;
        remember_detected_language(&mut cached, None, Some("en".into()), &[]);
        assert!(
            cached.is_none(),
            "hallucinated window must not lock language"
        );

        let speech = [TranscriptionSegment {
            text: "Bonjour à tous".into(),
            start_time: 0.0,
            end_time: 1.0,
            is_final: true,
            language: Some("fr".into()),
            confidence: Some(0.9),
            speaker: None,
        }];
        remember_detected_language(&mut cached, None, Some("fr".into()), &speech);
        assert_eq!(cached.as_deref(), Some("fr"));
    }

    #[test]
    fn explicit_language_is_not_overwritten_by_detect() {
        let mut cached = None;
        let speech = [TranscriptionSegment {
            text: "Hello".into(),
            start_time: 0.0,
            end_time: 1.0,
            is_final: true,
            language: Some("en".into()),
            confidence: Some(0.9),
            speaker: None,
        }];
        remember_detected_language(&mut cached, Some("fr"), Some("en".into()), &speech);
        assert!(cached.is_none());
    }

    #[test]
    fn silent_buffer_drops_any_segment() {
        let silent = vec![0.0f32; 16_000];
        assert!(pcm_rms(&silent) < SILENCE_RMS_FLOOR);
        let segs = vec![TranscriptionSegment {
            text: "Hello".into(),
            start_time: 0.0,
            end_time: 1.0,
            is_final: true,
            language: Some("en".into()),
            confidence: Some(0.9),
            speaker: None,
        }];
        assert!(drop_silent_or_hallucinated_window(segs, &silent).is_empty());
    }

    #[test]
    fn sole_thank_you_on_room_tone_is_dropped() {
        // Brown-ish noise above the digital-silence floor.
        let room: Vec<f32> = (0..16_000)
            .map(|i| ((i % 17) as f32 / 17.0 - 0.5) * 0.04)
            .collect();
        assert!(pcm_rms(&room) >= SILENCE_RMS_FLOOR);
        let segs = vec![TranscriptionSegment {
            text: "Thank you.".into(),
            start_time: 0.0,
            end_time: 1.0,
            is_final: true,
            language: Some("en".into()),
            confidence: Some(0.4),
            speaker: None,
        }];
        assert!(drop_silent_or_hallucinated_window(segs, &room).is_empty());
    }

    #[test]
    fn speech_window_with_thank_you_in_a_sentence_is_kept() {
        let speech: Vec<f32> = (0..16_000).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let segs = vec![TranscriptionSegment {
            text: "Thank you for joining the call.".into(),
            start_time: 0.0,
            end_time: 1.0,
            is_final: true,
            language: Some("en".into()),
            confidence: Some(0.9),
            speaker: None,
        }];
        let kept = drop_silent_or_hallucinated_window(segs, &speech);
        assert_eq!(kept.len(), 1);
        assert!(kept[0].text.contains("joining"));
    }

    #[test]
    fn silence_gate_on_flush_uses_unpadded_samples() {
        // Quiet speech tail above the floor; zero-padding to 1s dilutes RMS below it.
        let speech: Vec<f32> = (0..800).map(|i| (i as f32 * 0.1).sin() * 0.001).collect();
        assert!(pcm_rms(&speech) >= SILENCE_RMS_FLOOR);
        let mut padded = speech.clone();
        padded.resize(MIN_INFERENCE_SAMPLES, 0.0);
        assert!(pcm_rms(&padded) < SILENCE_RMS_FLOOR);

        let segs = vec![TranscriptionSegment {
            text: "okay".into(),
            start_time: 0.0,
            end_time: 0.05,
            is_final: true,
            language: Some("en".into()),
            confidence: Some(0.8),
            speaker: None,
        }];
        assert_eq!(
            drop_silent_or_hallucinated_window(segs.clone(), &speech).len(),
            1
        );
        assert!(drop_silent_or_hallucinated_window(segs, &padded).is_empty());
    }

    #[test]
    fn audio_requirements_deliver_100ms_without_shortening_final_windows() {
        let engine = WhisperEngine::new();
        let reqs = engine.audio_requirements();
        assert_eq!(reqs.sample_rate_hz, 16_000);
        assert_eq!(reqs.chunk_size_samples, CHUNK_SAMPLES as u32);
    }

    #[test]
    fn five_second_speech_waits_for_gap_or_seven() {
        let pcm: Vec<f32> = (0..WHISPER_SAMPLE_RATE as usize * 5)
            .map(|i| (i as f32 * 0.02).sin() * 0.3)
            .collect();
        assert_eq!(
            crate::engine::batch_windows::find_cut_samples(&pcm),
            None,
            "must not knife-cut at 5 s (data|platform / Snowflake)"
        );
    }
}
