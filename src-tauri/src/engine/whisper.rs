use std::path::{Path, PathBuf};

use tracing::{debug, info};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use super::{
    AudioInputRequirements, EngineError, TranscriptionEngine, TranscriptionSegment,
    collapse_whitespace,
};

/// Whisper sample rate: 16kHz mono f32 (required by whisper.cpp)
const WHISPER_SAMPLE_RATE: u32 = 16_000;

/// Number of CPU threads for whisper.cpp inference.
const WHISPER_N_THREADS: i32 = 4;

/// Chunk size for pipeline delivery: 5 seconds at 16kHz.
/// Non-overlapping chunks avoid duplicate text from sliding windows.
const CHUNK_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize * 5;

/// Minimum audio for meaningful inference (1 second).
const MIN_INFERENCE_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize;

/// Segments with no-speech probability above this are discarded.
const NO_SPEECH_PROB_THRESHOLD: f32 = 0.6;

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

struct LoadedWhisperModel {
    ctx: WhisperContext,
    #[allow(dead_code)]
    model_path: PathBuf,
}

/// Whisper STT engine via whisper-rs (whisper.cpp bindings).
/// Batch-oriented: accumulates audio, triggers inference every CHUNK_SAMPLES.
pub struct WhisperEngine {
    model: Option<LoadedWhisperModel>,
    /// Audio buffer — accumulates until chunk threshold
    audio_buffer: Vec<f32>,
    /// Cached language from first auto-detect inference.
    /// Subsequent chunks reuse the detected language for stable decoding.
    detected_language: Option<String>,
    /// Samples already sent to inference this session. whisper.cpp timestamps
    /// are relative to each window; this offsets them to session time so
    /// pauses and [H:MM] markers are meaningful across windows.
    consumed_samples: usize,
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
            audio_buffer: Vec::new(),
            detected_language: None,
            consumed_samples: 0,
        }
    }

    /// Session-time offset (seconds) for the next inference window,
    /// then advance by the window length.
    fn take_window_offset(&mut self, window_len: usize) -> f64 {
        let offset = self.consumed_samples as f64 / WHISPER_SAMPLE_RATE as f64;
        self.consumed_samples += window_len;
        offset
    }

    /// Run inference on a chunk of audio. Returns detected language code
    /// alongside the transcription segments.
    fn run_inference(
        ctx: &WhisperContext,
        audio: &[f32],
        language: Option<&str>,
    ) -> Result<(Vec<TranscriptionSegment>, Option<String>), EngineError> {
        if audio.is_empty() {
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
        self.audio_buffer.clear();

        info!("Whisper model unloaded");
        Ok(())
    }

    fn transcribe(
        &mut self,
        audio: &[f32],
        language: Option<&str>,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        if self.model.is_none() {
            return Err(EngineError::NotInitialized);
        }

        self.audio_buffer.extend_from_slice(audio);

        if self.audio_buffer.len() < CHUNK_SAMPLES {
            return Ok(vec![]);
        }

        let to_process: Vec<f32> = self.audio_buffer.drain(..).collect();
        let offset = self.take_window_offset(to_process.len());
        let loaded = self.model.as_ref().ok_or(EngineError::NotInitialized)?;

        // Use cached language if available, otherwise auto-detect
        let effective_lang = if language.is_some() {
            language.map(String::from)
        } else {
            self.detected_language.clone()
        };

        let (mut segments, detected) = Self::run_inference(
            &loaded.ctx,
            &to_process,
            effective_lang.as_deref().or(language),
        )?;
        for seg in &mut segments {
            seg.start_time += offset;
            seg.end_time += offset;
        }

        // Cache detected language from first successful auto-detect
        if language.is_none()
            && let Some(ref lang) = detected
            && self.detected_language.is_none()
        {
            info!(language = %lang, "Whisper auto-detected language, caching for session");
            self.detected_language = Some(lang.clone());
        }

        Ok(segments)
    }

    fn flush(&mut self) -> Result<Vec<TranscriptionSegment>, EngineError> {
        if self.model.is_none() {
            return Err(EngineError::NotInitialized);
        }

        if self.audio_buffer.len() < MIN_INFERENCE_SAMPLES {
            self.audio_buffer.clear();
            return Ok(vec![]);
        }

        let remaining: Vec<f32> = self.audio_buffer.drain(..).collect();
        let offset = self.take_window_offset(remaining.len());
        let loaded = self.model.as_ref().ok_or(EngineError::NotInitialized)?;
        let (mut segments, _) =
            Self::run_inference(&loaded.ctx, &remaining, self.detected_language.as_deref())?;
        for seg in &mut segments {
            seg.start_time += offset;
            seg.end_time += offset;
        }
        Ok(segments)
    }

    fn reset_state(&mut self) -> Result<(), EngineError> {
        self.audio_buffer.clear();
        // Clear cached language so next session auto-detects fresh
        self.detected_language = None;
        self.consumed_samples = 0;
        Ok(())
    }

    fn audio_requirements(&self) -> AudioInputRequirements {
        AudioInputRequirements {
            sample_rate_hz: WHISPER_SAMPLE_RATE,
            channels: 1,
            chunk_size_samples: CHUNK_SAMPLES as u32,
        }
    }

    fn normalize_text(&self, text: &str) -> String {
        strip_special_tokens(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
