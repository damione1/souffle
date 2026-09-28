mod audio_vad;
pub use audio_vad::SharedVadEngine;
pub(crate) mod learned_pair;
pub mod session_terms;
pub mod soundex;
mod text_dictionary;
mod text_filler;
mod text_stutter;
mod text_whitespace;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ── Typed enums (no magic strings) ─────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioFilterKind {
    SileroVad,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

pub(crate) fn pronunciation_aliases(term: &str, pronunciation: Option<&str>) -> Vec<String> {
    let term_lower = term.to_lowercase();
    let Some(raw) = pronunciation.map(str::trim).filter(|p| !p.is_empty()) else {
        return Vec::new();
    };
    raw.split(',')
        .map(str::trim)
        .filter(|alias| !alias.is_empty() && alias.to_lowercase() != term_lower)
        .map(ToString::to_string)
        .collect()
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

/// Load the Silero VAD model once so later sessions can reuse it through
/// [`build_audio_filters`] instead of rebuilding the ONNX session (SOU-286).
pub fn load_vad_engine(model_path: &Path) -> Result<SharedVadEngine, String> {
    crate::ort_runtime::ensure_ort_initialized();
    audio_vad::load_engine(model_path)
}

/// Build the session's audio gate. `shared_vad` is an engine already loaded
/// from `config.vad_model_path` (see [`load_vad_engine`]); when given it is
/// reset and reused, otherwise a new ONNX session is built for this call.
pub fn build_audio_filters(
    config: &PipelineConfig,
    source_sample_rate: u32,
    shared_vad: Option<SharedVadEngine>,
) -> AudioFilterChain {
    let mut filters: Vec<Box<dyn AudioFilter>> = Vec::new();
    if config.vad_enabled {
        if let Some(engine) = shared_vad {
            filters.push(Box::new(audio_vad::SileroVadFilter::from_engine(
                engine,
                source_sample_rate,
            )));
        } else if let Some(model_path) = &config.vad_model_path {
            match load_vad_engine(model_path) {
                Ok(engine) => filters.push(Box::new(audio_vad::SileroVadFilter::from_engine(
                    engine,
                    source_sample_rate,
                ))),
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
        && (!dictionary.is_empty() || !session_terms.is_empty() || !session_corrections.is_empty())
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
        // Single, bounded ort init for every Silero test (SOU-132): ort's own
        // lazy init must never race `ensure_ort_initialized` on another thread.
        crate::ort_runtime::ensure_ort_initialized_for_test();

        let config = PipelineConfig {
            vad_enabled: true,
            vad_model_path: Some(model_path),
            filler_removal_enabled: false,
            stutter_collapse_enabled: false,
            dictionary_correction_enabled: false,
        };
        let mut chain = build_audio_filters(&config, 16_000, None);
        // If the dylib failed to load, the VAD filter was silently skipped and
        // the empty chain would forward silence — which must be suppressed.
        let silence = vec![0.0f32; 480 * 4];
        assert!(
            !chain.process(&silence),
            "VAD did not gate silence — Silero filter missing (ort dylib load failed?)"
        );
    }

    /// Speech in the first 90 ms, silence in the last 480 ms: the block
    /// must be fed. Overwriting with the last 30 ms (SOU-067) returned
    /// silence once hangover expired inside the same call.
    #[test]
    fn silero_mixed_block_feeds_when_speech_is_not_last_frame() {
        let Some(model_path) = resolve_vad_model_path() else {
            eprintln!("skipping: silero_vad_v4.onnx not found");
            return;
        };
        if crate::ort_runtime::resolve_resource("libonnxruntime.dylib").is_none() {
            eprintln!("skipping: libonnxruntime.dylib not found");
            return;
        }
        // Single, bounded ort init for every Silero test (SOU-132): ort's own
        // lazy init must never race `ensure_ort_initialized` on another thread.
        crate::ort_runtime::ensure_ort_initialized_for_test();

        let mut vad = match audio_vad::SileroVadFilter::new(&model_path, 16_000) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: Silero VAD init failed: {e}");
                return;
            }
        };

        // 90 ms of a voiced-ish harmonic stack (onset is 2 frames), then
        // 480 ms of silence — hangover is 450 ms, so the last 30 ms is
        // true silence and would have overwritten the verdict.
        let mut block = Vec::with_capacity(480 * 19);
        for i in 0..(480 * 3) {
            let t = i as f32 / 16_000.0;
            block.push(
                (t * 120.0 * std::f32::consts::TAU).sin() * 0.35
                    + (t * 240.0 * std::f32::consts::TAU).sin() * 0.2
                    + (t * 360.0 * std::f32::consts::TAU).sin() * 0.1,
            );
        }
        block.extend(std::iter::repeat_n(0.0f32, 480 * 16));

        let mut probe = match audio_vad::SileroVadFilter::new(&model_path, 16_000) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: Silero VAD init failed: {e}");
                return;
            }
        };
        if !probe.process(&block[..480 * 3]) {
            eprintln!("skipping: synthetic voice not detected by Silero");
            return;
        }

        assert!(
            vad.process(&block),
            "mixed block with early speech must be fed (SOU-067 OR-accumulate)"
        );
    }

    /// SOU-286: a reused Silero engine must segment exactly like a fresh one.
    /// The fixture ends mid-speech, so without a full reset the second
    /// session would inherit the open gate (in_speech/hangover) and the LSTM
    /// state, and flag its leading silence as speech.
    #[test]
    fn reused_silero_engine_segments_like_a_fresh_session() {
        let Some(model_path) = resolve_vad_model_path() else {
            eprintln!("skipping: silero_vad_v4.onnx not found");
            return;
        };
        if crate::ort_runtime::resolve_resource("libonnxruntime.dylib").is_none() {
            eprintln!("skipping: libonnxruntime.dylib not found");
            return;
        }
        // Single, bounded ort init for every Silero test (SOU-132).
        crate::ort_runtime::ensure_ort_initialized_for_test();

        let config = PipelineConfig {
            vad_enabled: true,
            vad_model_path: Some(model_path.clone()),
            filler_removal_enabled: false,
            stutter_collapse_enabled: false,
            dictionary_correction_enabled: false,
        };

        // Real speech (SOU-030 TTS fixture: phrase, 1 s pause, phrase) behind
        // 300 ms of leading silence, in 30 ms blocks.
        let wav = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/audio/sou-030/hesitation-a-gap1000ms.wav");
        let mut reader = hound::WavReader::open(&wav).expect("open SOU-030 fixture");
        assert_eq!(reader.spec().sample_rate, 16_000, "fixture is 16 kHz");
        let mut fixture = vec![0.0f32; 480 * 10];
        fixture.extend(
            reader
                .samples::<i16>()
                .map(|s| f32::from(s.expect("fixture sample")) / 32_768.0),
        );

        let segment = |chain: &mut AudioFilterChain, audio: &[f32]| -> Vec<bool> {
            audio
                .chunks(480)
                .map(|block| chain.process(block))
                .collect()
        };

        let probe = segment(&mut build_audio_filters(&config, 16_000, None), &fixture);
        let Some(last_speech) = probe.iter().rposition(|&speech| speech) else {
            panic!("Silero detected no speech in the SOU-030 fixture");
        };
        // The last true block closes a hangover tail of true blocks; stop a
        // few blocks earlier so the session ends with the gate still open.
        fixture.truncate((last_speech - 4) * 480);

        let fresh = segment(&mut build_audio_filters(&config, 16_000, None), &fixture);
        assert!(!fresh[0], "fixture must start in silence");
        assert!(
            *fresh.last().expect("non-empty fixture"),
            "fixture must end mid-speech to exercise the reset"
        );

        let shared = match load_vad_engine(&model_path) {
            Ok(engine) => engine,
            Err(e) => panic!("Silero VAD init failed: {e}"),
        };
        let first = segment(
            &mut build_audio_filters(&config, 16_000, Some(std::sync::Arc::clone(&shared))),
            &fixture,
        );
        let second = segment(
            &mut build_audio_filters(&config, 16_000, Some(shared)),
            &fixture,
        );

        assert_eq!(first, fresh, "first session on a shared engine");
        assert_eq!(
            second, fresh,
            "second session must not inherit VAD state from the first"
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
