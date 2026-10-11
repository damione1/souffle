use std::path::Path;

use parakeet_rs::{ParakeetTDT, TimestampMode, Transcriber};
use tracing::{debug, info};

use super::batch_session::BatchSession;
use super::batch_windows::{CHUNK_SAMPLES, SAMPLE_RATE as PARAKEET_SAMPLE_RATE};
use super::{
    AudioInputRequirements, EngineError, TranscriptionEngine, TranscriptionSegment,
    collapse_whitespace,
};

/// Minimum audio worth an inference pass on flush (0.5 second).
const MIN_INFERENCE_SAMPLES: usize = PARAKEET_SAMPLE_RATE as usize / 2;

/// NVIDIA Parakeet TDT engine via parakeet-rs (ONNX Runtime, CPU).
/// Batch-oriented: same silence-gap windows as Whisper ([4 s, 7 s], else 7 s).
/// Revisable snapshots share the loaded model; final windows remain unchanged.
///
/// Uses the bundled ONNX Runtime dylib through ort's load-dynamic mode —
/// see `crate::ort_runtime` for why static linking is forbidden here.
pub struct ParakeetEngine {
    model: Option<ParakeetTDT>,
    session: BatchSession,
}

impl Default for ParakeetEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ParakeetEngine {
    pub fn new() -> Self {
        Self {
            model: None,
            session: BatchSession::default(),
        }
    }

    fn run_inference(
        model: &mut ParakeetTDT,
        audio: Vec<f32>,
        offset: f64,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        if audio.is_empty() || audio.iter().all(|s| *s == 0.0) {
            return Ok(vec![]);
        }

        let result = model
            .transcribe_samples(
                audio,
                PARAKEET_SAMPLE_RATE,
                1,
                Some(TimestampMode::Sentences),
            )
            .map_err(|e| EngineError::InferenceError(format!("Parakeet inference: {e}")))?;

        if crate::debug::transcription_debug_enabled() {
            debug!(
                target: crate::logging::TRANSCRIPT_TARGET,
                sentences = result.tokens.len(),
                text = %result.text,
                "Parakeet inference complete"
            );
        }

        let mut segments: Vec<TranscriptionSegment> = result
            .tokens
            .iter()
            .filter(|token| !token.text.trim().is_empty())
            .map(|token| TranscriptionSegment {
                text: token.text.clone(),
                start_time: token.start as f64 + offset,
                end_time: token.end as f64 + offset,
                is_final: true,
                language: None,
                confidence: None,
                speaker: None,
            })
            .collect();

        // Sentence grouping can come back empty even when text was decoded
        if segments.is_empty() && !result.text.trim().is_empty() {
            segments.push(TranscriptionSegment {
                text: result.text,
                start_time: offset,
                end_time: offset,
                is_final: true,
                language: None,
                confidence: None,
                speaker: None,
            });
        }

        Ok(segments)
    }
}

impl TranscriptionEngine for ParakeetEngine {
    fn load_model(&mut self, model_path: &Path) -> Result<(), EngineError> {
        crate::ort_runtime::ensure_ort_initialized();

        info!(path = %model_path.display(), "Loading Parakeet TDT model");

        // None = parakeet-rs defaults: CPU execution provider, 4 intra-op
        // threads. CoreML is slower than CPU for these dynamic-shape graphs.
        let model = ParakeetTDT::from_pretrained(model_path, None)
            .map_err(|e| EngineError::LoadError(format!("Parakeet model load: {e}")))?;

        self.model = Some(model);
        info!("Parakeet TDT model loaded");
        Ok(())
    }

    fn unload_model(&mut self) -> Result<(), EngineError> {
        self.model = None;
        self.session.reset(false);
        info!("Parakeet TDT model unloaded");
        Ok(())
    }

    fn transcribe(
        &mut self,
        audio: &[f32],
        _language: Option<&str>,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let model = self.model.as_mut().ok_or(EngineError::NotInitialized)?;
        self.session.transcribe(audio, None, |pcm, _, _| {
            Ok((Self::run_inference(model, pcm.to_vec(), 0.0)?, None))
        })
    }

    fn transcribe_dual(
        &mut self,
        mic: &[f32],
        system: &[f32],
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let model = self.model.as_mut().ok_or(EngineError::NotInitialized)?;
        self.session.dual(mic, system, |pcm, _, _| {
            Ok((Self::run_inference(model, pcm.to_vec(), 0.0)?, None))
        })
    }

    fn flush(&mut self) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let model = self.model.as_mut().ok_or(EngineError::NotInitialized)?;
        self.session.flush(MIN_INFERENCE_SAMPLES, |pcm, _, _| {
            Ok((Self::run_inference(model, pcm.to_vec(), 0.0)?, None))
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
            sample_rate_hz: PARAKEET_SAMPLE_RATE,
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
        collapse_whitespace(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_engines_support_source_lanes() {
        assert!(ParakeetEngine::new().supports_diarization());
        assert!(crate::engine::whisper::WhisperEngine::new().supports_diarization());
    }

    #[test]
    fn transcribe_without_load_returns_error() {
        let mut engine = ParakeetEngine::new();
        assert!(engine.transcribe(&[0.0f32; 16_000], None).is_err());
    }

    #[test]
    fn flush_without_load_returns_error() {
        let mut engine = ParakeetEngine::new();
        assert!(engine.flush().is_err());
    }

    #[test]
    fn reset_clears_buffer_without_model() {
        let mut engine = ParakeetEngine::new();
        assert!(engine.reset_state().is_ok());
    }

    #[test]
    fn audio_requirements_are_16khz_100ms_hops() {
        let engine = ParakeetEngine::new();
        let reqs = engine.audio_requirements();
        assert_eq!(reqs.sample_rate_hz, 16_000);
        assert_eq!(reqs.channels, 1);
        assert_eq!(reqs.chunk_size_samples, CHUNK_SAMPLES as u32);
    }

    #[test]
    fn five_second_boundary_does_not_cut_mid_phrase() {
        // Continuous speech through 5 s — the old knife-edge that split
        // "The next | checkpoint" and made TDT invent a completion.
        let five_seconds = PARAKEET_SAMPLE_RATE as usize * 5;
        let pcm: Vec<f32> = (0..five_seconds)
            .map(|i| (i as f32 * 0.02).sin() * 0.3)
            .collect();
        assert_eq!(
            crate::engine::batch_windows::find_cut_samples(&pcm),
            None,
            "must wait past 5 s so 'next checkpoint' stays in one window"
        );

        let mut longer = pcm;
        longer.extend((0..five_seconds).map(|i| (i as f32 * 0.02).sin() * 0.3));
        let cut = crate::engine::batch_windows::find_cut_samples(&longer).unwrap();
        assert!(cut > five_seconds);
        assert_eq!(cut, 16_000 * 7);
    }

    /// End-to-end inference against the real downloaded model. Run with:
    ///   say -o /tmp/parakeet_test.wav --data-format=LEF32@16000 "Hello world..."
    ///   cargo test parakeet_real_inference -- --ignored --nocapture
    /// Requires the model files in the app models dir and the bundled ort dylib.
    #[test]
    #[ignore = "requires downloaded Parakeet model (~670MB) and a test WAV"]
    fn parakeet_real_inference() {
        let profile = crate::engine::resolve_transcription_profile(
            Some(crate::engine::PARAKEET_ENGINE_ID),
            Some(crate::engine::PARAKEET_MODEL_TDT_06B_V3_ID),
            Some(crate::engine::ORT_BACKEND_ID),
        )
        .unwrap();
        let model_dir = crate::models::model_dir(&profile);
        assert!(
            crate::models::model_exists(&profile),
            "model files missing in {}",
            model_dir.display()
        );

        let mut reader = hound::WavReader::open("/tmp/parakeet_test.wav")
            .expect("test WAV missing — synthesize one with `say` first");
        assert_eq!(reader.spec().sample_rate, 16_000);
        let mut samples: Vec<f32> = reader.samples::<f32>().filter_map(|s| s.ok()).collect();
        // Pad to a full window so transcribe() triggers inference
        samples.resize(PARAKEET_SAMPLE_RATE as usize * 5, 0.0);

        let mut engine = ParakeetEngine::new();
        engine.load_model(&model_dir).expect("load model");
        let segments = engine.transcribe(&samples, None).expect("transcribe");
        let text: String = segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        eprintln!("Parakeet transcription: {text:?}");
        assert!(text.contains("hello"), "expected 'hello' in: {text:?}");
        assert!(
            text.contains("transcription"),
            "expected 'transcription' in: {text:?}"
        );
    }
}
