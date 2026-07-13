pub mod kyutai;
pub mod parakeet;
pub mod whisper;

#[cfg(any(test, feature = "test-support"))]
pub mod mock;

use std::path::{Path, PathBuf};

pub const KYUTAI_ENGINE_ID: &str = "kyutai";
pub const KYUTAI_MODEL_ID: &str = "stt-1b-en_fr";
pub const KYUTAI_MODEL_2_6B_ID: &str = "stt-2.6b-en";
pub const WHISPER_ENGINE_ID: &str = "whisper";
pub const WHISPER_MODEL_TURBO_ID: &str = "turbo";
pub const PARAKEET_ENGINE_ID: &str = "parakeet";
pub const PARAKEET_MODEL_TDT_06B_V3_ID: &str = "parakeet-tdt-0.6b-v3";
pub const CANDLE_BACKEND_ID: &str = "candle";
pub const WHISPER_RS_BACKEND_ID: &str = "whisper-rs";
pub const CTRANSLATE2_BACKEND_ID: &str = "ctranslate2";
pub const ORT_BACKEND_ID: &str = "onnx-ort";

const KYUTAI_1B_CANDLE_ARTIFACT_ID: &str = "hf-candle-stt-1b-en-fr";
const KYUTAI_2_6B_CANDLE_ARTIFACT_ID: &str = "hf-candle-stt-2-6b-en";
const WHISPER_TURBO_GGML_ARTIFACT_ID: &str = "hf-ggml-large-v3-turbo";
const PARAKEET_TDT_06B_V3_ONNX_ARTIFACT_ID: &str = "hf-onnx-parakeet-tdt-0-6b-v3";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, specta::Type)]
pub struct TranscriptionProfileSelection {
    pub engine_id: String,
    pub model_id: String,
    pub backend_id: String,
}

impl Default for TranscriptionProfileSelection {
    fn default() -> Self {
        Self {
            engine_id: KYUTAI_ENGINE_ID.to_string(),
            model_id: KYUTAI_MODEL_ID.to_string(),
            backend_id: CANDLE_BACKEND_ID.to_string(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, specta::Type)]
pub struct TranscriptionProfile {
    pub engine_id: String,
    pub engine_label: String,
    pub model_id: String,
    pub model_label: String,
    #[serde(default = "default_backend_id_string")]
    pub backend_id: String,
    #[serde(default = "default_backend_label_string")]
    pub backend_label: String,
}

impl Default for TranscriptionProfile {
    fn default() -> Self {
        default_transcription_profile()
    }
}

impl TranscriptionProfile {
    pub fn from_legacy_engine(engine_label: &str) -> Self {
        let trimmed = engine_label.trim();
        if trimmed.is_empty()
            || trimmed.eq_ignore_ascii_case("kyutai")
            || trimmed.contains("Kyutai")
        {
            return default_transcription_profile();
        }

        Self {
            engine_id: slug_id(trimmed),
            engine_label: trimmed.to_string(),
            model_id: "legacy".to_string(),
            model_label: "Legacy profile".to_string(),
            backend_id: default_backend_id_string(),
            backend_label: default_backend_label_string(),
        }
    }

    pub fn selection(&self) -> TranscriptionProfileSelection {
        TranscriptionProfileSelection {
            engine_id: self.engine_id.clone(),
            model_id: self.model_id.clone(),
            backend_id: self.backend_id.clone(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, specta::Type)]
pub struct TranscriptionCapabilities {
    pub supports_streaming: bool,
    pub supports_batch_transcription: bool,
    pub supports_language_auto_detect: bool,
    pub supports_word_timestamps: bool,
    pub supports_partial_results: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, specta::Type)]
pub struct AudioInputRequirements {
    pub sample_rate_hz: u32,
    pub channels: u8,
    pub chunk_size_samples: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, specta::Type)]
pub struct ModelArtifactDescriptor {
    pub id: String,
    pub label: String,
    pub description: String,
    pub provider: String,
    pub repository: String,
    pub revision: Option<String>,
    pub file_format: String,
    pub download_size_bytes: Option<u64>,
    pub required_files: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, specta::Type)]
pub struct TranscriptionRuntimeBackendDescriptor {
    pub id: String,
    pub label: String,
    pub description: String,
    pub recommended: bool,
    pub available_in_app: bool,
    pub availability_note: Option<String>,
    pub artifacts: Vec<ModelArtifactDescriptor>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, specta::Type)]
pub struct TranscriptionModelDescriptor {
    pub id: String,
    pub label: String,
    pub description: String,
    pub download_size_bytes: Option<u64>,
    pub recommended_memory_bytes: Option<u64>,
    pub supported_languages: Vec<String>,
    pub capabilities: TranscriptionCapabilities,
    pub audio_input: AudioInputRequirements,
    pub available_in_app: bool,
    pub availability_note: Option<String>,
    pub backends: Vec<TranscriptionRuntimeBackendDescriptor>,
    pub recommended_backend_id: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, specta::Type)]
pub struct TranscriptionEngineDescriptor {
    pub id: String,
    pub label: String,
    pub description: String,
    pub models: Vec<TranscriptionModelDescriptor>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, specta::Type)]
pub struct TranscriptionCatalog {
    pub engines: Vec<TranscriptionEngineDescriptor>,
    pub selected_engine_id: String,
    pub selected_model_id: String,
    pub selected_backend_id: String,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionRuntimePhase {
    DownloadRequired,
    LoadRequired,
    Ready,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, specta::Type)]
pub struct TranscriptionRuntimeStatus {
    pub profile: TranscriptionProfile,
    pub phase: TranscriptionRuntimePhase,
    pub model_dir: String,
}

pub fn transcription_runtime_phase(downloaded: bool, loaded: bool) -> TranscriptionRuntimePhase {
    if loaded {
        TranscriptionRuntimePhase::Ready
    } else if downloaded {
        TranscriptionRuntimePhase::LoadRequired
    } else {
        TranscriptionRuntimePhase::DownloadRequired
    }
}

/// Runtime interface implemented by each engine family/backend pair.
/// Product metadata belongs in descriptors, not on the runtime itself.
/// Optional streaming context diagnostics for engines with a finite LM window
/// (Kyutai). Exposed on heartbeats so long-meeting freezes are legible in logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextWindowStats {
    pub context_frames: usize,
    pub frames_since_refresh: usize,
    pub refresh_count: u64,
}

/// Methods take &mut self and there is no Send/Sync bound: engines are
/// created, used, and dropped on the engine actor thread only.
pub trait TranscriptionEngine {
    fn load_model(&mut self, model_path: &Path) -> Result<(), EngineError>;
    fn unload_model(&mut self) -> Result<(), EngineError>;
    fn transcribe(
        &mut self,
        audio: &[f32],
        language: Option<&str>,
    ) -> Result<Vec<TranscriptionSegment>, EngineError>;
    fn flush(&mut self) -> Result<Vec<TranscriptionSegment>, EngineError>;
    fn reset_state(&mut self) -> Result<(), EngineError>;
    /// Audio format requirements for this engine's inference pipeline.
    /// Used by the audio capture thread (target sample rate) and the
    /// inference pipeline (chunk size) to adapt to each engine.
    fn audio_requirements(&self) -> AudioInputRequirements;
    /// Gain factor applied to raw microphone input before inference.
    fn mic_gain(&self) -> f32 {
        1.0
    }
    /// Seconds by which this engine's emitted words lag behind the audio
    /// that produced them (e.g. Kyutai's ASR delay). Used to size the
    /// single-stream VAD drain window: how long to keep feeding audio to
    /// the engine after speech stops, so a trailing word doesn't stay stuck
    /// behind the next utterance. 0 for engines with no such lag.
    fn emission_delay_seconds(&self) -> f64 {
        0.0
    }
    /// Normalize engine-specific tokens from transcribed text.
    /// Called by the pipeline on every segment before emitting to the frontend.
    /// Each engine overrides this to strip its own special tokens
    /// (e.g., SentencePiece `▁` for Kyutai, `[_TT_xxx]` for Whisper).
    fn normalize_text(&self, text: &str) -> String {
        text.to_string()
    }

    /// Whether this engine can transcribe two synchronized audio streams (mic +
    /// system audio) and label each segment by speaker. Only streaming engines
    /// that support a batch dimension (Kyutai/moshi) can; others run meetings as
    /// a single mixed stream with no Me/Them labels.
    fn supports_diarization(&self) -> bool {
        false
    }

    /// Enable/disable diarized (two-stream) mode. Takes effect on the next
    /// `reset_state` (which rebuilds the engine's streaming state with the right
    /// batch size). No-op for engines that don't support diarization.
    fn set_diarization(&mut self, _enabled: bool) {}

    /// Heuristic meeting-language prior for LID and lane resets (Kyutai only).
    /// Never passed to the engine as a forced decode language.
    fn set_meeting_language_prior(&mut self, _prior: crate::settings::MeetingTranscriptionLanguage) {}

    /// Transcribe one paired frame from both streams (mic = Me, system = Them),
    /// returning segments already tagged with their speaker. Only valid after
    /// `set_diarization(true)` + `reset_state` on an engine that supports it.
    fn transcribe_dual(
        &mut self,
        _me: &[f32],
        _them: &[f32],
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        Err(EngineError::InferenceError(
            "diarization not supported by this engine".into(),
        ))
    }

    /// Finite LM context window stats when the engine tracks them (Kyutai).
    fn context_window_stats(&self) -> Option<ContextWindowStats> {
        None
    }
}

/// Who produced a segment in a diarized meeting: the microphone is the local
/// user (Me), system audio is everyone else (Them), or a persistent speaker
/// identity resolved by offline diarization (`Persistent`, keyed by the
/// `speakers.id` row). `None` = single-stream session (dictation, or a
/// meeting recorded without diarization).
///
/// Wire/DB encoding is always a plain string: "me", "them", or "spk:<id>".
/// Serialize/Deserialize/specta::Type are implemented by hand below (instead
/// of derived) so the `Persistent` variant's payload still round-trips
/// through a single string on the wire, and the generated TypeScript type is
/// a plain `string` rather than a tagged union.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Speaker {
    Me,
    Them,
    /// A persistent, cross-meeting speaker identity. The `i64` is the
    /// `speakers.id` primary key.
    Persistent(i64),
}

impl Speaker {
    pub fn as_str(self) -> String {
        match self {
            Speaker::Me => "me".to_string(),
            Speaker::Them => "them".to_string(),
            Speaker::Persistent(id) => format!("spk:{id}"),
        }
    }

    pub fn parse(s: &str) -> Option<Speaker> {
        match s {
            "me" => Some(Speaker::Me),
            "them" => Some(Speaker::Them),
            other => other
                .strip_prefix("spk:")
                .and_then(|id| id.parse::<i64>().ok())
                .map(Speaker::Persistent),
        }
    }
}

impl serde::Serialize for Speaker {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Speaker {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Speaker::parse(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid speaker: {s:?}")))
    }
}

/// Manual impl (rather than `#[derive(specta::Type)]`) so the generated
/// TypeScript type is a plain `string` ("me" | "them" | `spk:<id>`), matching
/// the hand-written `Serialize`/`Deserialize` above.
impl specta::Type for Speaker {
    fn inline(
        type_map: &mut specta::TypeCollection,
        generics: specta::Generics,
    ) -> specta::DataType {
        <String as specta::Type>::inline(type_map, generics)
    }
}

/// A piece of transcribed text with metadata
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct TranscriptionSegment {
    pub text: String,
    pub start_time: f64,
    pub end_time: f64,
    pub is_final: bool,
    pub language: Option<String>,
    pub confidence: Option<f32>,
    /// Set by the pipeline for diarized meetings; `None` otherwise.
    #[serde(default)]
    pub speaker: Option<Speaker>,
}

/// Collapse runs of whitespace to single spaces and trim.
/// Shared by engine normalize_text implementations.
pub fn collapse_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut last_was_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                result.push(' ');
            }
            last_was_space = true;
        } else {
            result.push(ch);
            last_was_space = false;
        }
    }
    result.trim().to_string()
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
    vec![
        TranscriptionEngineDescriptor {
            id: KYUTAI_ENGINE_ID.to_string(),
            label: "Kyutai".to_string(),
            description: "Local speech-to-text family optimized for live transcription with Mimi audio tokenization.".to_string(),
            models: vec![kyutai_1b_model_descriptor(), kyutai_2_6b_model_descriptor()],
        },
        TranscriptionEngineDescriptor {
            id: WHISPER_ENGINE_ID.to_string(),
            label: "Whisper".to_string(),
            description: "Multilingual speech recognition family with strong batch transcription support.".to_string(),
            models: vec![whisper_turbo_model_descriptor()],
        },
        TranscriptionEngineDescriptor {
            id: PARAKEET_ENGINE_ID.to_string(),
            label: "Parakeet".to_string(),
            description: "NVIDIA speech recognition family with fast multilingual transcription, punctuation, and capitalization.".to_string(),
            models: vec![parakeet_tdt_06b_v3_model_descriptor()],
        },
    ]
}

pub fn default_transcription_profile() -> TranscriptionProfile {
    resolve_transcription_profile(
        Some(KYUTAI_ENGINE_ID),
        Some(KYUTAI_MODEL_ID),
        Some(CANDLE_BACKEND_ID),
    )
    .expect("default transcription profile must resolve")
}

pub fn resolve_transcription_profile(
    engine_id: Option<&str>,
    model_id: Option<&str>,
    backend_id: Option<&str>,
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
        .unwrap_or_else(|| {
            engine
                .models
                .first()
                .map(|model| model.id.as_str())
                .unwrap_or(KYUTAI_MODEL_ID)
        });
    let model = engine
        .models
        .iter()
        .find(|candidate| candidate.id == model_id)
        .ok_or_else(|| {
            format!("Unknown transcription model '{model_id}' for engine '{engine_id}'")
        })?;

    let backend_id = backend_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(model.recommended_backend_id.as_str());
    let backend = model
        .backends
        .iter()
        .find(|candidate| candidate.id == backend_id)
        .ok_or_else(|| {
            format!(
                "Unknown transcription backend '{backend_id}' for model '{engine_id}:{model_id}'"
            )
        })?;
    if !model.available_in_app {
        return Err(unavailable_profile_error(engine, model, backend));
    }
    if !backend.available_in_app {
        return Err(unavailable_profile_error(engine, model, backend));
    }

    Ok(TranscriptionProfile {
        engine_id: engine.id.clone(),
        engine_label: engine.label.clone(),
        model_id: model.id.clone(),
        model_label: model.label.clone(),
        backend_id: backend.id.clone(),
        backend_label: backend.label.clone(),
    })
}

pub fn resolve_transcription_selection(
    selection: &TranscriptionProfileSelection,
) -> Result<TranscriptionProfile, String> {
    resolve_transcription_profile(
        Some(&selection.engine_id),
        Some(&selection.model_id),
        Some(&selection.backend_id),
    )
}

pub fn resolve_transcription_artifact(
    profile: &TranscriptionProfile,
) -> Result<ModelArtifactDescriptor, String> {
    let catalog = transcription_engine_catalog();
    let engine = catalog
        .iter()
        .find(|candidate| candidate.id == profile.engine_id)
        .ok_or_else(|| format!("Unknown transcription engine '{}'", profile.engine_id))?;
    let model = engine
        .models
        .iter()
        .find(|candidate| candidate.id == profile.model_id)
        .ok_or_else(|| {
            format!(
                "Unknown transcription model '{}' for engine '{}'",
                profile.model_id, profile.engine_id
            )
        })?;
    let backend = model
        .backends
        .iter()
        .find(|candidate| candidate.id == profile.backend_id)
        .ok_or_else(|| {
            format!(
                "Unknown transcription backend '{}' for model '{}:{}'",
                profile.backend_id, profile.engine_id, profile.model_id
            )
        })?;
    backend
        .artifacts
        .first()
        .cloned()
        .ok_or_else(|| format!("No artifacts registered for '{}'", backend.id))
}

pub fn create_engine(
    profile: &TranscriptionProfile,
) -> Result<Box<dyn TranscriptionEngine>, String> {
    match (profile.engine_id.as_str(), profile.backend_id.as_str()) {
        (KYUTAI_ENGINE_ID, CANDLE_BACKEND_ID) => Ok(Box::new(kyutai::KyutaiEngine::new())),
        (WHISPER_ENGINE_ID, WHISPER_RS_BACKEND_ID) => Ok(Box::new(whisper::WhisperEngine::new())),
        (PARAKEET_ENGINE_ID, ORT_BACKEND_ID) => Ok(Box::new(parakeet::ParakeetEngine::new())),
        _ => Err(format!(
            "No runtime implementation registered for '{}:{}'",
            profile.engine_id, profile.backend_id
        )),
    }
}

fn kyutai_1b_model_descriptor() -> TranscriptionModelDescriptor {
    TranscriptionModelDescriptor {
        id: KYUTAI_MODEL_ID.to_string(),
        label: "STT 1B FR/EN".to_string(),
        description: "Fast Kyutai streaming model tuned for French and English dictation."
            .to_string(),
        download_size_bytes: Some(2_400_000_000),
        recommended_memory_bytes: Some(4_000_000_000),
        supported_languages: vec!["fr".to_string(), "en".to_string()],
        capabilities: kyutai_streaming_capabilities(),
        audio_input: kyutai_audio_requirements(),
        available_in_app: true,
        availability_note: None,
        backends: vec![kyutai_candle_backend(
            KYUTAI_1B_CANDLE_ARTIFACT_ID,
            "Hugging Face Candle export for the Kyutai 1B FR/EN model.",
            "kyutai/stt-1b-en_fr-candle",
            Some(2_400_000_000),
        )],
        recommended_backend_id: CANDLE_BACKEND_ID.to_string(),
    }
}

fn kyutai_2_6b_model_descriptor() -> TranscriptionModelDescriptor {
    TranscriptionModelDescriptor {
        id: KYUTAI_MODEL_2_6B_ID.to_string(),
        label: "STT 2.6B EN".to_string(),
        description: "Larger Kyutai streaming model optimized for English transcription quality."
            .to_string(),
        download_size_bytes: Some(5_620_000_000),
        recommended_memory_bytes: Some(10_000_000_000),
        supported_languages: vec!["en".to_string()],
        capabilities: kyutai_streaming_capabilities(),
        audio_input: kyutai_audio_requirements(),
        available_in_app: true,
        availability_note: None,
        backends: vec![kyutai_candle_backend(
            KYUTAI_2_6B_CANDLE_ARTIFACT_ID,
            "Hugging Face Candle export for the Kyutai 2.6B EN model.",
            "kyutai/stt-2.6b-en-candle",
            Some(5_620_000_000),
        )],
        recommended_backend_id: CANDLE_BACKEND_ID.to_string(),
    }
}

fn kyutai_streaming_capabilities() -> TranscriptionCapabilities {
    TranscriptionCapabilities {
        supports_streaming: true,
        supports_batch_transcription: false,
        supports_language_auto_detect: true,
        supports_word_timestamps: true,
        supports_partial_results: true,
    }
}

fn kyutai_audio_requirements() -> AudioInputRequirements {
    AudioInputRequirements {
        sample_rate_hz: 24_000,
        channels: 1,
        chunk_size_samples: crate::constants::MIMI_FRAME_SIZE as u32,
    }
}

fn kyutai_candle_backend(
    artifact_id: &str,
    artifact_description: &str,
    repository: &str,
    download_size_bytes: Option<u64>,
) -> TranscriptionRuntimeBackendDescriptor {
    TranscriptionRuntimeBackendDescriptor {
        id: CANDLE_BACKEND_ID.to_string(),
        label: "Candle".to_string(),
        description: "Pure Rust runtime used by Souffle for local transcription.".to_string(),
        recommended: true,
        available_in_app: true,
        availability_note: None,
        artifacts: vec![ModelArtifactDescriptor {
            id: artifact_id.to_string(),
            label: "Hugging Face".to_string(),
            description: artifact_description.to_string(),
            provider: "huggingface".to_string(),
            repository: repository.to_string(),
            revision: None,
            file_format: "safetensors".to_string(),
            download_size_bytes,
            required_files: vec!["config.json".to_string(), "model.safetensors".to_string()],
        }],
    }
}

fn whisper_turbo_model_descriptor() -> TranscriptionModelDescriptor {
    TranscriptionModelDescriptor {
        id: WHISPER_MODEL_TURBO_ID.to_string(),
        label: "Large V3 Turbo".to_string(),
        description:
            "Fast multilingual Whisper model. Batch transcription with Metal acceleration."
                .to_string(),
        download_size_bytes: Some(1_620_000_000),
        recommended_memory_bytes: Some(3_000_000_000),
        supported_languages: vec!["multilingual".to_string()],
        capabilities: TranscriptionCapabilities {
            supports_streaming: false,
            supports_batch_transcription: true,
            supports_language_auto_detect: true,
            supports_word_timestamps: true,
            supports_partial_results: false,
        },
        audio_input: AudioInputRequirements {
            sample_rate_hz: 16_000,
            channels: 1,
            chunk_size_samples: 16_000 * 5,
        },
        available_in_app: true,
        availability_note: None,
        backends: vec![whisper_rs_backend()],
        recommended_backend_id: WHISPER_RS_BACKEND_ID.to_string(),
    }
}

fn whisper_rs_backend() -> TranscriptionRuntimeBackendDescriptor {
    TranscriptionRuntimeBackendDescriptor {
        id: WHISPER_RS_BACKEND_ID.to_string(),
        label: "whisper.cpp".to_string(),
        description: "whisper.cpp via whisper-rs with Metal GPU acceleration.".to_string(),
        recommended: true,
        available_in_app: true,
        availability_note: None,
        artifacts: vec![ModelArtifactDescriptor {
            id: WHISPER_TURBO_GGML_ARTIFACT_ID.to_string(),
            label: "Hugging Face".to_string(),
            description: "GGML F16 weights for whisper-large-v3-turbo.".to_string(),
            provider: "huggingface".to_string(),
            repository: "ggerganov/whisper.cpp".to_string(),
            revision: None,
            file_format: "ggml".to_string(),
            download_size_bytes: Some(1_620_000_000),
            required_files: vec!["ggml-large-v3-turbo.bin".to_string()],
        }],
    }
}

fn parakeet_tdt_06b_v3_model_descriptor() -> TranscriptionModelDescriptor {
    TranscriptionModelDescriptor {
        id: PARAKEET_MODEL_TDT_06B_V3_ID.to_string(),
        label: "TDT 0.6B v3".to_string(),
        description: "Multilingual Parakeet model (25 languages incl. French and English) with punctuation and capitalization. Quantized int8, CPU inference.".to_string(),
        download_size_bytes: Some(672_000_000),
        recommended_memory_bytes: Some(3_000_000_000),
        supported_languages: vec!["multilingual".to_string()],
        capabilities: TranscriptionCapabilities {
            supports_streaming: false,
            supports_batch_transcription: true,
            supports_language_auto_detect: true,
            supports_word_timestamps: true,
            supports_partial_results: false,
        },
        audio_input: AudioInputRequirements {
            sample_rate_hz: 16_000,
            channels: 1,
            chunk_size_samples: 16_000 * 5,
        },
        available_in_app: true,
        availability_note: None,
        backends: vec![parakeet_ort_backend()],
        recommended_backend_id: ORT_BACKEND_ID.to_string(),
    }
}

fn parakeet_ort_backend() -> TranscriptionRuntimeBackendDescriptor {
    TranscriptionRuntimeBackendDescriptor {
        id: ORT_BACKEND_ID.to_string(),
        label: "ONNX Runtime".to_string(),
        description: "ONNX Runtime via parakeet-rs, sharing the app's bundled dynamic runtime."
            .to_string(),
        recommended: true,
        available_in_app: true,
        availability_note: None,
        artifacts: vec![ModelArtifactDescriptor {
            id: PARAKEET_TDT_06B_V3_ONNX_ARTIFACT_ID.to_string(),
            label: "Hugging Face".to_string(),
            description: "Community ONNX export (int8) of NVIDIA parakeet-tdt-0.6b-v3.".to_string(),
            provider: "huggingface".to_string(),
            repository: "istupakov/parakeet-tdt-0.6b-v3-onnx".to_string(),
            revision: None,
            file_format: "onnx".to_string(),
            download_size_bytes: Some(672_000_000),
            required_files: vec![
                "encoder-model.int8.onnx".to_string(),
                "decoder_joint-model.int8.onnx".to_string(),
                "vocab.txt".to_string(),
            ],
        }],
    }
}

fn unavailable_profile_error(
    engine: &TranscriptionEngineDescriptor,
    model: &TranscriptionModelDescriptor,
    backend: &TranscriptionRuntimeBackendDescriptor,
) -> String {
    model
        .availability_note
        .clone()
        .or_else(|| backend.availability_note.clone())
        .unwrap_or_else(|| {
            format!(
                "'{} • {} • {}' is declared in the catalog but not available in this app build",
                engine.label, model.label, backend.label
            )
        })
}

fn default_backend_id_string() -> String {
    CANDLE_BACKEND_ID.to_string()
}

fn default_backend_label_string() -> String {
    "Candle".to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_uses_kyutai_1b_candle() {
        let p = default_transcription_profile();
        assert_eq!(p.engine_id, KYUTAI_ENGINE_ID);
        assert_eq!(p.model_id, KYUTAI_MODEL_ID);
        assert_eq!(p.backend_id, CANDLE_BACKEND_ID);
    }

    #[test]
    fn catalog_contains_both_kyutai_models() {
        let cat = transcription_engine_catalog();
        let kyutai = cat.iter().find(|e| e.id == KYUTAI_ENGINE_ID).unwrap();
        assert!(kyutai.models.iter().any(|m| m.id == KYUTAI_MODEL_ID));
        assert!(kyutai.models.iter().any(|m| m.id == KYUTAI_MODEL_2_6B_ID));
        assert!(cat.iter().any(|e| e.id == WHISPER_ENGINE_ID));
        assert!(cat.iter().any(|e| e.id == PARAKEET_ENGINE_ID));
    }

    #[test]
    fn resolve_profile_defaults() {
        let p = resolve_transcription_profile(None, None, None).unwrap();
        assert_eq!(p.engine_id, KYUTAI_ENGINE_ID);
        assert_eq!(p.model_id, KYUTAI_MODEL_ID);
        assert_eq!(p.backend_id, CANDLE_BACKEND_ID);
    }

    #[test]
    fn resolve_profile_valid() {
        let p = resolve_transcription_profile(
            Some(KYUTAI_ENGINE_ID),
            Some(KYUTAI_MODEL_2_6B_ID),
            Some(CANDLE_BACKEND_ID),
        )
        .unwrap();
        assert_eq!(p.engine_id, KYUTAI_ENGINE_ID);
        assert_eq!(p.model_id, KYUTAI_MODEL_2_6B_ID);
        assert_eq!(p.backend_id, CANDLE_BACKEND_ID);
    }

    #[test]
    fn resolve_profile_unknown_engine() {
        let r = resolve_transcription_profile(
            Some("nonexistent"),
            Some(KYUTAI_MODEL_ID),
            Some(CANDLE_BACKEND_ID),
        );
        assert!(r.is_err());
    }

    #[test]
    fn resolve_profile_unknown_model() {
        let r = resolve_transcription_profile(
            Some(KYUTAI_ENGINE_ID),
            Some("nonexistent"),
            Some(CANDLE_BACKEND_ID),
        );
        assert!(r.is_err());
    }

    #[test]
    fn resolve_profile_unknown_backend() {
        let r = resolve_transcription_profile(
            Some(KYUTAI_ENGINE_ID),
            Some(KYUTAI_MODEL_ID),
            Some("mlx"),
        );
        assert!(r.is_err());
    }

    #[test]
    fn resolve_profile_whisper_turbo() {
        let p = resolve_transcription_profile(
            Some(WHISPER_ENGINE_ID),
            Some(WHISPER_MODEL_TURBO_ID),
            Some(WHISPER_RS_BACKEND_ID),
        )
        .unwrap();
        assert_eq!(p.engine_id, WHISPER_ENGINE_ID);
        assert_eq!(p.model_id, WHISPER_MODEL_TURBO_ID);
        assert_eq!(p.backend_id, WHISPER_RS_BACKEND_ID);
    }

    #[test]
    fn resolve_profile_rejects_old_whisper_ctranslate2_backend() {
        let r = resolve_transcription_profile(
            Some(WHISPER_ENGINE_ID),
            Some(WHISPER_MODEL_TURBO_ID),
            Some(CTRANSLATE2_BACKEND_ID),
        );
        assert!(r.is_err());
    }

    #[test]
    fn resolve_profile_parakeet_tdt_06b_v3() {
        let p = resolve_transcription_profile(
            Some(PARAKEET_ENGINE_ID),
            Some(PARAKEET_MODEL_TDT_06B_V3_ID),
            Some(ORT_BACKEND_ID),
        )
        .unwrap();
        assert_eq!(p.engine_id, PARAKEET_ENGINE_ID);
        assert_eq!(p.model_id, PARAKEET_MODEL_TDT_06B_V3_ID);
        assert_eq!(p.backend_id, ORT_BACKEND_ID);
    }

    #[test]
    fn create_engine_parakeet() {
        let profile = resolve_transcription_profile(
            Some(PARAKEET_ENGINE_ID),
            Some(PARAKEET_MODEL_TDT_06B_V3_ID),
            Some(ORT_BACKEND_ID),
        )
        .unwrap();
        let engine = create_engine(&profile).unwrap();
        let reqs = engine.audio_requirements();
        assert_eq!(reqs.sample_rate_hz, 16_000);
        assert_eq!(reqs.chunk_size_samples, 16_000 * 5);
    }

    #[test]
    fn parakeet_artifact_has_int8_onnx_files() {
        let profile = resolve_transcription_profile(
            Some(PARAKEET_ENGINE_ID),
            Some(PARAKEET_MODEL_TDT_06B_V3_ID),
            Some(ORT_BACKEND_ID),
        )
        .unwrap();
        let artifact = resolve_transcription_artifact(&profile).unwrap();
        assert_eq!(artifact.file_format, "onnx");
        assert!(
            artifact
                .required_files
                .iter()
                .any(|f| f == "encoder-model.int8.onnx")
        );
        assert!(
            artifact
                .required_files
                .iter()
                .any(|f| f == "decoder_joint-model.int8.onnx")
        );
        assert!(artifact.required_files.iter().any(|f| f == "vocab.txt"));
        // config.json must NOT be listed: it would trigger the Kyutai-specific
        // config-discovery path in the downloader.
        assert!(!artifact.required_files.iter().any(|f| f == "config.json"));
    }

    #[test]
    fn resolve_profile_trims_whitespace() {
        let p = resolve_transcription_profile(
            Some("  kyutai  "),
            Some("  stt-1b-en_fr  "),
            Some("  candle  "),
        )
        .unwrap();
        assert_eq!(p.engine_id, KYUTAI_ENGINE_ID);
        assert_eq!(p.model_id, KYUTAI_MODEL_ID);
        assert_eq!(p.backend_id, CANDLE_BACKEND_ID);
    }

    #[test]
    fn resolve_profile_empty_uses_defaults() {
        let p = resolve_transcription_profile(Some(""), Some(""), Some("")).unwrap();
        assert_eq!(p.engine_id, KYUTAI_ENGINE_ID);
        assert_eq!(p.model_id, KYUTAI_MODEL_ID);
        assert_eq!(p.backend_id, CANDLE_BACKEND_ID);
    }

    #[test]
    fn resolve_artifact_for_kyutai_2_6b() {
        let profile = resolve_transcription_profile(
            Some(KYUTAI_ENGINE_ID),
            Some(KYUTAI_MODEL_2_6B_ID),
            Some(CANDLE_BACKEND_ID),
        )
        .unwrap();
        let artifact = resolve_transcription_artifact(&profile).unwrap();
        assert_eq!(artifact.repository, "kyutai/stt-2.6b-en-candle");
    }

    #[test]
    fn create_engine_kyutai() {
        let profile = default_transcription_profile();
        let e = create_engine(&profile);
        assert!(e.is_ok());
    }

    #[test]
    fn create_engine_whisper() {
        let profile = resolve_transcription_profile(
            Some(WHISPER_ENGINE_ID),
            Some(WHISPER_MODEL_TURBO_ID),
            Some(WHISPER_RS_BACKEND_ID),
        )
        .unwrap();
        let e = create_engine(&profile);
        assert!(e.is_ok());
    }

    #[test]
    fn whisper_engine_audio_requirements() {
        let engine = whisper::WhisperEngine::new();
        let reqs = engine.audio_requirements();
        assert_eq!(reqs.sample_rate_hz, 16_000);
        assert_eq!(reqs.channels, 1);
        assert!(reqs.chunk_size_samples > 0);
    }

    #[test]
    fn whisper_artifact_has_ggml_file() {
        let profile = resolve_transcription_profile(
            Some(WHISPER_ENGINE_ID),
            Some(WHISPER_MODEL_TURBO_ID),
            Some(WHISPER_RS_BACKEND_ID),
        )
        .unwrap();
        let artifact = resolve_transcription_artifact(&profile).unwrap();
        assert_eq!(artifact.file_format, "ggml");
        assert!(artifact.required_files.iter().any(|f| f.ends_with(".bin")));
    }

    #[test]
    fn create_engine_unknown_backend() {
        let profile = TranscriptionProfile {
            backend_id: "unknown".into(),
            backend_label: "Unknown".into(),
            ..default_transcription_profile()
        };
        let e = create_engine(&profile);
        assert!(e.is_err());
    }

    #[test]
    fn slug_id_basic() {
        let p = TranscriptionProfile::from_legacy_engine("My Custom Engine");
        assert_eq!(p.engine_id, "my-custom-engine");
    }

    #[test]
    fn slug_id_consecutive_specials() {
        let p = TranscriptionProfile::from_legacy_engine("Engine!!!Version");
        assert_eq!(p.engine_id, "engine-version");
    }

    #[test]
    fn slug_id_empty() {
        let p = TranscriptionProfile::from_legacy_engine("");
        assert_eq!(p.engine_id, KYUTAI_ENGINE_ID);
    }

    #[test]
    fn from_legacy_engine_empty() {
        let p = TranscriptionProfile::from_legacy_engine("");
        assert_eq!(p.engine_id, KYUTAI_ENGINE_ID);
        assert_eq!(p.model_id, KYUTAI_MODEL_ID);
        assert_eq!(p.backend_id, CANDLE_BACKEND_ID);
    }

    #[test]
    fn from_legacy_engine_kyutai_case() {
        let p = TranscriptionProfile::from_legacy_engine("Kyutai STT");
        assert_eq!(p.engine_id, KYUTAI_ENGINE_ID);
    }

    #[test]
    fn from_legacy_engine_custom() {
        let p = TranscriptionProfile::from_legacy_engine("Some Custom Engine");
        assert!(!p.engine_id.is_empty());
        assert_eq!(p.engine_id, "some-custom-engine");
        assert_eq!(p.model_id, "legacy");
        assert_eq!(p.backend_id, CANDLE_BACKEND_ID);
    }

    #[test]
    fn whisper_engine_reset_clears_buffer() {
        let mut engine = whisper::WhisperEngine::new();
        // reset_state should not fail on a fresh engine
        assert!(engine.reset_state().is_ok());
    }

    #[test]
    fn whisper_engine_flush_without_load_returns_error() {
        let mut engine = whisper::WhisperEngine::new();
        assert!(engine.flush().is_err());
    }

    #[test]
    fn whisper_engine_transcribe_without_load_returns_error() {
        let mut engine = whisper::WhisperEngine::new();
        let audio = vec![0.0f32; 16_000];
        assert!(engine.transcribe(&audio, None).is_err());
    }

    #[test]
    fn create_engine_whisper_returns_correct_audio_requirements() {
        let profile = resolve_transcription_profile(
            Some(WHISPER_ENGINE_ID),
            Some(WHISPER_MODEL_TURBO_ID),
            Some(WHISPER_RS_BACKEND_ID),
        )
        .unwrap();
        let engine = create_engine(&profile).unwrap();
        let reqs = engine.audio_requirements();
        assert_eq!(reqs.sample_rate_hz, 16_000);
        assert_ne!(
            reqs.chunk_size_samples,
            crate::constants::MIMI_FRAME_SIZE as u32
        );
    }

    #[test]
    fn create_engine_kyutai_returns_correct_audio_requirements() {
        let profile = default_transcription_profile();
        let engine = create_engine(&profile).unwrap();
        let reqs = engine.audio_requirements();
        assert_eq!(reqs.sample_rate_hz, crate::constants::SAMPLE_RATE);
        assert_eq!(
            reqs.chunk_size_samples,
            crate::constants::MIMI_FRAME_SIZE as u32
        );
    }
}
