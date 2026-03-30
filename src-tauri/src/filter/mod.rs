mod audio_vad;
pub mod soundex;
mod text_dictionary;
mod text_filler;
mod text_stutter;
mod text_whitespace;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ── Typed enums (no magic strings) ─────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum AudioFilterKind {
    SileroVad,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TextFilterKind {
    FillerRemoval,
    StutterCollapse,
    WhitespaceNormalization,
    DictionaryCorrection,
}

// ── Contracts ──────────────────────────────────────────

/// Audio-level gate applied BEFORE engine sees audio.
/// Returns true = forward frame, false = suppress.
pub trait AudioFilter: Send {
    fn kind(&self) -> AudioFilterKind;
    fn process(&mut self, audio: &[f32]) -> bool;
    fn reset(&mut self);
}

/// Text-level transform applied AFTER engine transcription.
pub trait TextFilter: Send {
    fn kind(&self) -> TextFilterKind;
    fn apply(&self, text: &str) -> String;
}

// ── Composable Chains ──────────────────────────────────

pub struct AudioFilterChain {
    filters: Vec<Box<dyn AudioFilter>>,
}

impl AudioFilterChain {
    pub fn new(filters: Vec<Box<dyn AudioFilter>>) -> Self {
        Self { filters }
    }

    /// ALL filters must pass for the frame to be forwarded.
    pub fn process(&mut self, audio: &[f32]) -> bool {
        self.filters.iter_mut().all(|f| f.process(audio))
    }

    pub fn reset(&mut self) {
        for f in &mut self.filters {
            f.reset();
        }
    }
}

pub struct TextFilterChain {
    filters: Vec<Box<dyn TextFilter>>,
}

impl TextFilterChain {
    pub fn new(filters: Vec<Box<dyn TextFilter>>) -> Self {
        Self { filters }
    }

    /// Sequential transform, short-circuit on empty.
    pub fn apply(&self, text: &str) -> String {
        let mut result = text.to_string();
        for f in &self.filters {
            if result.is_empty() {
                return result;
            }
            result = f.apply(&result);
        }
        result
    }
}

// ── Pipeline Config DTO ────────────────────────────────

#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub vad_enabled: bool,
    pub vad_model_path: Option<PathBuf>,
    pub filler_removal_enabled: bool,
    pub stutter_collapse_enabled: bool,
    pub dictionary_correction_enabled: bool,
}

// ── Dictionary DTO ─────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct DictionaryEntry {
    pub id: i64,
    pub term: String,
    pub phonetic_code: Option<String>,
    pub category: Option<String>,
    pub created_at: String,
}

// ── VAD model path resolution ──────────────────────────

const VAD_MODEL_FILENAME: &str = "silero_vad_v4.onnx";

/// Resolve the Silero VAD model file path.
/// Checks multiple locations in order:
/// 1. Tauri bundled resources (production): next to the binary under resources/
/// 2. Development: src-tauri/resources/ relative to the binary
/// 3. App data directory
pub fn resolve_vad_model_path() -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()))
        .map(|bin_dir| {
            vec![
                // Production Tauri app: resources bundled next to binary
                bin_dir.join("resources").join(VAD_MODEL_FILENAME),
                // macOS .app bundle: ../Resources/resources/
                bin_dir
                    .join("../Resources/resources")
                    .join(VAD_MODEL_FILENAME),
                // Dev mode: src-tauri/resources/ (cargo tauri dev runs from src-tauri/)
                PathBuf::from("resources").join(VAD_MODEL_FILENAME),
            ]
        })
        .unwrap_or_default();

    for path in &candidates {
        if path.exists() {
            tracing::info!(path = %path.display(), "Found Silero VAD model");
            return Some(path.clone());
        }
    }

    tracing::warn!(
        "Silero VAD model not found in any of: {:?}",
        candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>()
    );
    None
}

// ── Factory functions ──────────────────────────────────

pub fn build_audio_filters(config: &PipelineConfig, source_sample_rate: u32) -> AudioFilterChain {
    let mut filters: Vec<Box<dyn AudioFilter>> = Vec::new();
    if config.vad_enabled {
        if let Some(model_path) = &config.vad_model_path {
            match audio_vad::SileroVadFilter::new(model_path, source_sample_rate) {
                Ok(vad) => filters.push(Box::new(vad)),
                Err(e) => {
                    tracing::warn!("Failed to create Silero VAD filter, skipping: {e}");
                }
            }
        } else {
            tracing::warn!("VAD enabled but no model path configured, skipping");
        }
    }
    AudioFilterChain::new(filters)
}

pub fn build_text_filters(
    config: &PipelineConfig,
    dictionary: Vec<DictionaryEntry>,
) -> TextFilterChain {
    let mut filters: Vec<Box<dyn TextFilter>> = Vec::new();
    if config.filler_removal_enabled {
        filters.push(Box::new(text_filler::FillerRemovalFilter::new()));
    }
    if config.stutter_collapse_enabled {
        filters.push(Box::new(text_stutter::StutterCollapseFilter::new()));
    }
    if config.dictionary_correction_enabled && !dictionary.is_empty() {
        filters.push(Box::new(text_dictionary::DictionaryFilter::new(dictionary)));
    }
    // Whitespace normalization always runs last to clean up artifacts from previous filters
    filters.push(Box::new(text_whitespace::WhitespaceNormFilter));
    TextFilterChain::new(filters)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_chain_empty_always_passes() {
        let mut chain = AudioFilterChain::new(vec![]);
        assert!(chain.process(&[0.0; 512]));
    }

    #[test]
    fn text_chain_empty_passes_through() {
        let chain = TextFilterChain::new(vec![]);
        assert_eq!(chain.apply("hello"), "hello");
    }

    #[test]
    fn text_chain_short_circuits_on_empty() {
        let chain = TextFilterChain::new(vec![
            Box::new(text_filler::FillerRemovalFilter::new()),
            Box::new(text_whitespace::WhitespaceNormFilter),
        ]);
        assert_eq!(chain.apply(""), "");
    }

    #[test]
    fn build_text_filters_includes_whitespace_always() {
        let config = PipelineConfig {
            vad_enabled: false,
            vad_model_path: None,
            filler_removal_enabled: false,
            stutter_collapse_enabled: false,
            dictionary_correction_enabled: false,
        };
        let chain = build_text_filters(&config, vec![]);
        // Whitespace normalization should still clean up
        assert_eq!(chain.apply("  hello   world  "), "hello world");
    }
}
