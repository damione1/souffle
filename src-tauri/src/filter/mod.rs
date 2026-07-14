mod audio_vad;
pub mod session_terms;
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
    /// How the term sounds when spoken, spelled out (e.g. "vésix" for "V6").
    /// Drives phonetic matching; when absent, the term's own Soundex is used
    /// (except for digit-bearing terms, whose Soundex is meaningless).
    pub pronunciation: Option<String>,
    pub category: Option<String>,
    pub created_at: String,
}

// ── VAD model path resolution ──────────────────────────

const VAD_MODEL_FILENAME: &str = "silero_vad_v4.onnx";

/// Resolve the Silero VAD model file path.
pub fn resolve_vad_model_path() -> Option<PathBuf> {
    let path = crate::ort_runtime::resolve_resource(VAD_MODEL_FILENAME);
    if let Some(ref p) = path {
        tracing::info!(path = %p.display(), "Found Silero VAD model");
    } else {
        tracing::warn!("Silero VAD model ({VAD_MODEL_FILENAME}) not found");
    }
    path
}

// ── Factory functions ──────────────────────────────────

pub fn build_audio_filters(config: &PipelineConfig, source_sample_rate: u32) -> AudioFilterChain {
    let mut filters: Vec<Box<dyn AudioFilter>> = Vec::new();
    if config.vad_enabled {
        crate::ort_runtime::ensure_ort_initialized();
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
    session_terms: &[String],
    session_corrections: &[session_terms::SessionCorrection],
) -> TextFilterChain {
    let mut filters: Vec<Box<dyn TextFilter>> = Vec::new();
    if config.filler_removal_enabled {
        filters.push(Box::new(text_filler::FillerRemovalFilter::new()));
    }
    if config.stutter_collapse_enabled {
        filters.push(Box::new(text_stutter::StutterCollapseFilter::new()));
    }
    if config.dictionary_correction_enabled
        && (!dictionary.is_empty()
            || !session_terms.is_empty()
            || !session_corrections.is_empty())
    {
        let filter = if session_corrections.is_empty() {
            text_dictionary::DictionaryFilter::with_session_terms(dictionary, session_terms)
        } else {
            text_dictionary::DictionaryFilter::with_session_hints(
                dictionary,
                session_terms,
                session_corrections,
            )
        };
        filters.push(Box::new(filter));
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

    /// Regression check for the bundled ONNX Runtime dylib: actually loads it
    /// via ort load-dynamic and runs Silero VAD inference. Catches dylib/API
    /// version mismatches (e.g. ort api-24 vs an older bundled runtime).
    /// Skips when the bundled resources are not present (e.g. bare CI).
    #[test]
    fn silero_vad_runs_against_bundled_ort_dylib() {
        let Some(model_path) = resolve_vad_model_path() else {
            eprintln!("skipping: silero_vad_v4.onnx not found");
            return;
        };
        if crate::ort_runtime::resolve_resource("libonnxruntime.dylib").is_none() {
            eprintln!("skipping: libonnxruntime.dylib not found");
            return;
        }

        let config = PipelineConfig {
            vad_enabled: true,
            vad_model_path: Some(model_path),
            filler_removal_enabled: false,
            stutter_collapse_enabled: false,
            dictionary_correction_enabled: false,
        };
        let mut chain = build_audio_filters(&config, 16_000);
        // If the dylib failed to load, the VAD filter was silently skipped and
        // the empty chain would forward silence — which must be suppressed.
        let silence = vec![0.0f32; 480 * 4];
        assert!(
            !chain.process(&silence),
            "VAD did not gate silence — Silero filter missing (ort dylib load failed?)"
        );
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
        let chain = build_text_filters(&config, vec![], &[], &[]);
        // Whitespace normalization should still clean up
        assert_eq!(chain.apply("  hello   world  "), "hello world");
    }

    #[test]
    fn session_corrections_apply_when_dictionary_correction_enabled() {
        use crate::filter::session_terms::SessionCorrection;
        let config = PipelineConfig {
            vad_enabled: false,
            vad_model_path: None,
            filler_removal_enabled: false,
            stutter_collapse_enabled: false,
            dictionary_correction_enabled: true,
        };
        let chain = build_text_filters(
            &config,
            vec![],
            &[],
            &[SessionCorrection {
                misspelling: "Kubernetis".to_string(),
                term: "Kubernetes".to_string(),
            }],
        );
        assert_eq!(chain.apply("Kubernetis cluster"), "Kubernetes cluster");
    }

    #[test]
    fn session_corrections_skipped_when_dictionary_correction_disabled() {
        use crate::filter::session_terms::SessionCorrection;
        let config = PipelineConfig {
            vad_enabled: false,
            vad_model_path: None,
            filler_removal_enabled: false,
            stutter_collapse_enabled: false,
            dictionary_correction_enabled: false,
        };
        let chain = build_text_filters(
            &config,
            vec![],
            &[],
            &[SessionCorrection {
                misspelling: "Kubernetis".to_string(),
                term: "Kubernetes".to_string(),
            }],
        );
        assert_eq!(chain.apply("Kubernetis cluster"), "Kubernetis cluster");
    }
}
