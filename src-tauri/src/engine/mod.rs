pub mod kyutai;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const KYUTAI_ENGINE_ID: &str = "kyutai";
pub const KYUTAI_MODEL_ID: &str = "stt-1b-en_fr";

pub type SharedTranscriptionEngine = Arc<Mutex<Box<dyn TranscriptionEngine>>>;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TranscriptionProfile {
    pub engine_id: String,
    pub engine_label: String,
    pub model_id: String,
    pub model_label: String,
}

impl Default for TranscriptionProfile {
    fn default() -> Self {
        default_transcription_profile()
    }
}

impl TranscriptionProfile {
    pub fn from_legacy_engine(engine_label: &str) -> Self {
        let trimmed = engine_label.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("kyutai") || trimmed.contains("Kyutai") {
            return default_transcription_profile();
        }

        Self {
            engine_id: slug_id(trimmed),
            engine_label: trimmed.to_string(),
            model_id: "legacy".to_string(),
            model_label: "Legacy profile".to_string(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TranscriptionModelDescriptor {
    pub id: String,
    pub label: String,
    pub description: String,
    pub download_size_bytes: Option<u64>,
    pub supported_languages: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TranscriptionEngineDescriptor {
    pub id: String,
    pub label: String,
    pub description: String,
    pub supports_streaming: bool,
    pub models: Vec<TranscriptionModelDescriptor>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TranscriptionCatalog {
    pub engines: Vec<TranscriptionEngineDescriptor>,
    pub selected_engine_id: String,
    pub selected_model_id: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TranscriptionRuntimeStatus {
    pub profile: TranscriptionProfile,
    pub downloaded: bool,
    pub loaded: bool,
    pub model_dir: String,
}

/// Core trait that ALL transcription engines must implement.
/// Adding a new engine (Whisper, Parakeet, Kyutai, future models)
/// means implementing this trait — nothing else changes.
pub trait TranscriptionEngine: Send + Sync {
    /// Human-readable engine name for UI display
    fn name(&self) -> &str;

    /// Supported languages as ISO 639-1 codes
    fn supported_languages(&self) -> Vec<String>;

    /// Whether this engine supports true streaming (token-by-token)
    /// vs chunk-based processing (e.g., Whisper's 30s windows)
    fn supports_streaming(&self) -> bool;

    /// Initialize the engine, load model weights into memory.
    fn load_model(&mut self, model_path: &Path) -> Result<(), EngineError>;

    /// Unload model from memory. Must free all GPU/CPU memory.
    fn unload_model(&mut self) -> Result<(), EngineError>;

    /// Process an audio chunk and return transcription segments.
    /// `audio`: raw PCM f32 samples at 24kHz mono.
    /// `language`: optional language hint (None = auto-detect).
    fn transcribe(
        &self,
        audio: &[f32],
        language: Option<&str>,
    ) -> Result<Vec<TranscriptionSegment>, EngineError>;

    /// For streaming engines: signal that audio input has ended.
    /// Returns any remaining buffered segments.
    fn flush(&self) -> Result<Vec<TranscriptionSegment>, EngineError>;

    /// Reset internal state between transcription sessions.
    /// Clears KV caches, positional encodings, and any accumulated buffers.
    fn reset_state(&self) -> Result<(), EngineError>;

    /// Estimated VRAM/RAM usage in bytes for the loaded model.
    fn memory_usage(&self) -> Option<u64>;
}

/// A piece of transcribed text with metadata
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TranscriptionSegment {
    pub text: String,
    pub start_time: f64,
    pub end_time: f64,
    pub is_final: bool,
    pub language: Option<String>,
    pub confidence: Option<f32>,
}

/// Errors that engines can produce
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("Model not found at path: {0}")]
    ModelNotFound(PathBuf),
    #[error("Failed to load model: {0}")]
    LoadError(String),
    #[error("Inference failed: {0}")]
    InferenceError(String),
    #[error("Unsupported language: {0}")]
    UnsupportedLanguage(String),
    #[error("Engine not initialized")]
    NotInitialized,
    #[error("Out of memory")]
    OutOfMemory,
}

pub fn transcription_engine_catalog() -> Vec<TranscriptionEngineDescriptor> {
    vec![TranscriptionEngineDescriptor {
        id: KYUTAI_ENGINE_ID.to_string(),
        label: "Kyutai".to_string(),
        description: "Streaming local STT with Candle + Mimi at 24kHz.".to_string(),
        supports_streaming: true,
        models: vec![TranscriptionModelDescriptor {
            id: KYUTAI_MODEL_ID.to_string(),
            label: "STT 1B FR/EN".to_string(),
            description: "stt-1b-en_fr local model from Kyutai.".to_string(),
            download_size_bytes: Some(2_400_000_000),
            supported_languages: vec!["fr".to_string(), "en".to_string()],
        }],
    }]
}

pub fn default_transcription_profile() -> TranscriptionProfile {
    TranscriptionProfile {
        engine_id: KYUTAI_ENGINE_ID.to_string(),
        engine_label: "Kyutai".to_string(),
        model_id: KYUTAI_MODEL_ID.to_string(),
        model_label: "STT 1B FR/EN".to_string(),
    }
}

pub fn resolve_transcription_profile(
    engine_id: Option<&str>,
    model_id: Option<&str>,
) -> Result<TranscriptionProfile, String> {
    let catalog = transcription_engine_catalog();
    let engine_id = engine_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(KYUTAI_ENGINE_ID);
    let engine = catalog
        .iter()
        .find(|candidate| candidate.id == engine_id)
        .ok_or_else(|| format!("Unknown transcription engine '{engine_id}'"))?;
    let model_id = model_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| engine.models.first().map(|model| model.id.as_str()).unwrap_or(KYUTAI_MODEL_ID));
    let model = engine
        .models
        .iter()
        .find(|candidate| candidate.id == model_id)
        .ok_or_else(|| format!("Unknown transcription model '{model_id}' for engine '{engine_id}'"))?;

    Ok(TranscriptionProfile {
        engine_id: engine.id.clone(),
        engine_label: engine.label.clone(),
        model_id: model.id.clone(),
        model_label: model.label.clone(),
    })
}

pub fn create_engine(engine_id: &str) -> Result<Box<dyn TranscriptionEngine>, String> {
    match engine_id {
        KYUTAI_ENGINE_ID => Ok(Box::new(kyutai::KyutaiEngine::new())),
        _ => Err(format!("No engine implementation registered for '{engine_id}'")),
    }
}

pub fn default_transcription_engine() -> Box<dyn TranscriptionEngine> {
    Box::new(kyutai::KyutaiEngine::new())
}

fn slug_id(value: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }

    slug.trim_matches('-').to_string()
}
