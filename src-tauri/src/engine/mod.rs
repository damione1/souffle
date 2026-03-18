use std::path::{Path, PathBuf};

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
    /// `audio`: raw PCM f32 samples at 16kHz mono.
    /// `language`: optional language hint (None = auto-detect).
    fn transcribe(
        &self,
        audio: &[f32],
        language: Option<&str>,
    ) -> Result<Vec<TranscriptionSegment>, EngineError>;

    /// For streaming engines: signal that audio input has ended.
    /// Returns any remaining buffered segments.
    fn flush(&self) -> Result<Vec<TranscriptionSegment>, EngineError>;

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
