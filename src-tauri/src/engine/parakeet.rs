use std::path::Path;

use parakeet_rs::{ParakeetTDT, TimestampMode, Transcriber};
use tracing::{debug, info};

use super::{
    AudioInputRequirements, EngineError, TranscriptionEngine, TranscriptionSegment,
    collapse_whitespace,
};

/// Parakeet sample rate: 16kHz mono f32 (fixed by the model's mel frontend)
const PARAKEET_SAMPLE_RATE: u32 = 16_000;

/// Chunk size for pipeline delivery: 5 seconds at 16kHz.
/// Non-overlapping windows, same shape as the Whisper engine — the model
/// transcribes each window independently (TDT inference is stateless).
const CHUNK_SAMPLES: usize = PARAKEET_SAMPLE_RATE as usize * 5;

/// Minimum audio worth an inference pass on flush (0.5 second).
const MIN_INFERENCE_SAMPLES: usize = PARAKEET_SAMPLE_RATE as usize / 2;

/// NVIDIA Parakeet TDT engine via parakeet-rs (ONNX Runtime, CPU).
/// Batch-oriented: accumulates audio, runs inference every CHUNK_SAMPLES.
///
/// Uses the bundled ONNX Runtime dylib through ort's load-dynamic mode —
/// see `crate::ort_runtime` for why static linking is forbidden here.
pub struct ParakeetEngine {
    model: Option<ParakeetTDT>,
    audio_buffer: Vec<f32>,
    /// Samples already sent to inference this session. Token timestamps are
    /// relative to each window; this offsets them to session time.
    consumed_samples: usize,
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
            audio_buffer: Vec::new(),
            consumed_samples: 0,
        }
    }

    /// Session-time offset (seconds) for the next inference window,
    /// then advance by the window length.
    fn take_window_offset(&mut self, window_len: usize) -> f64 {
        let offset = self.consumed_samples as f64 / PARAKEET_SAMPLE_RATE as f64;
        self.consumed_samples += window_len;
        offset
    }

    fn run_inference(
        model: &mut ParakeetTDT,
        audio: Vec<f32>,
        offset: f64,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        if audio.is_empty() {
            return Ok(vec![]);
        }

        let result = model
            .transcribe_samples(audio, PARAKEET_SAMPLE_RATE, 1, Some(TimestampMode::Sentences))
            .map_err(|e| EngineError::InferenceError(format!("Parakeet inference: {e}")))?;

        if crate::debug::transcription_debug_enabled() {
            debug!(
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
        self.audio_buffer.clear();
        info!("Parakeet TDT model unloaded");
        Ok(())
    }

    fn transcribe(
        &mut self,
        audio: &[f32],
        _language: Option<&str>,
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
        let model = self.model.as_mut().ok_or(EngineError::NotInitialized)?;
        Self::run_inference(model, to_process, offset)
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
        let model = self.model.as_mut().ok_or(EngineError::NotInitialized)?;
        Self::run_inference(model, remaining, offset)
    }

    fn reset_state(&mut self) -> Result<(), EngineError> {
        // TDT inference is stateless per window; only our buffer carries over.
        self.audio_buffer.clear();
        self.consumed_samples = 0;
        Ok(())
    }

    fn audio_requirements(&self) -> AudioInputRequirements {
        AudioInputRequirements {
            sample_rate_hz: PARAKEET_SAMPLE_RATE,
            channels: 1,
            chunk_size_samples: CHUNK_SAMPLES as u32,
        }
    }

    fn normalize_text(&self, text: &str) -> String {
        collapse_whitespace(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn audio_requirements_are_16khz_5s_windows() {
        let engine = ParakeetEngine::new();
        let reqs = engine.audio_requirements();
        assert_eq!(reqs.sample_rate_hz, 16_000);
        assert_eq!(reqs.channels, 1);
        assert_eq!(reqs.chunk_size_samples, 80_000);
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
        samples.resize(CHUNK_SAMPLES, 0.0);

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
        assert!(text.contains("transcription"), "expected 'transcription' in: {text:?}");
    }
}
