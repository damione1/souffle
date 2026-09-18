//! Transcription model tab (SOU-188 milestone 6, AC2). The download/switch
//! state machine itself is never reimplemented here: every transition goes
//! through the same commands the Svelte controller calls
//! (`get_model_status`, `download_model`, `load_model`), this module only
//! builds the picker's flat option list and formats display strings - port
//! of `features/transcription/catalog.ts`'s pure functions.

use crate::MainWindow;
use souffle_lib::engine::{
    TranscriptionCatalog, TranscriptionModelDescriptor, TranscriptionProfileSelection,
    TranscriptionRuntimeBackendDescriptor, TranscriptionRuntimePhase,
};

pub struct FlatModelOption {
    pub engine_id: String,
    pub model_id: String,
    pub backend_id: String,
    pub label: String,
}

impl FlatModelOption {
    pub fn selection(&self) -> TranscriptionProfileSelection {
        TranscriptionProfileSelection {
            engine_id: self.engine_id.clone(),
            model_id: self.model_id.clone(),
            backend_id: self.backend_id.clone(),
        }
    }
}

fn backend_available(backend: &TranscriptionRuntimeBackendDescriptor) -> bool {
    backend.available_in_app
}

fn first_available_backend(
    model: &TranscriptionModelDescriptor,
) -> Option<&TranscriptionRuntimeBackendDescriptor> {
    model
        .backends
        .iter()
        .find(|b| backend_available(b))
        .or_else(|| model.backends.first())
}

pub fn model_available(model: &TranscriptionModelDescriptor) -> bool {
    model.available_in_app && model.backends.iter().any(backend_available)
}

/// Port of `listAvailableModelOptions()`.
pub fn list_available_model_options(catalog: &TranscriptionCatalog) -> Vec<FlatModelOption> {
    catalog
        .engines
        .iter()
        .flat_map(|engine| {
            engine
                .models
                .iter()
                .filter(|model| model_available(model))
                .filter_map(|model| {
                    let backend = first_available_backend(model)?;
                    Some(FlatModelOption {
                        engine_id: engine.id.clone(),
                        model_id: model.id.clone(),
                        backend_id: backend.id.clone(),
                        label: format!("{} \u{2014} {}", engine.label, model.label),
                    })
                })
        })
        .collect()
}

pub fn find_option<'a>(options: &'a [FlatModelOption], label: &str) -> Option<&'a FlatModelOption> {
    options.iter().find(|o| o.label == label)
}

pub fn phase_label(phase: TranscriptionRuntimePhase) -> &'static str {
    match phase {
        TranscriptionRuntimePhase::DownloadRequired => "Téléchargement requis",
        TranscriptionRuntimePhase::LoadRequired => "Chargement requis",
        TranscriptionRuntimePhase::Ready => "Prêt",
    }
}

/// `0` is "never unload" - a distinct wording, not `crate::audio_ui::
/// minute_label(0)`'s "0 min".
pub fn unload_timeout_label(value: u32) -> String {
    if value == 0 {
        "Jamais".to_string()
    } else {
        crate::audio_ui::minute_label(value)
    }
}

pub fn parse_unload_timeout_label(label: &str) -> Option<u32> {
    if label == "Jamais" {
        Some(0)
    } else {
        crate::audio_ui::parse_minute_label(label)
    }
}

/// Pushes the picker's option list + current selection, and the unload-
/// timeout picker. Status text/busy/download state are pushed separately
/// by whatever triggers a transition (settings-open, or a model switch),
/// since they depend on a live `get_model_status` call the caller already
/// made.
pub fn populate_options(
    window: &MainWindow,
    catalog: &TranscriptionCatalog,
    model_unload_timeout_minutes: u32,
    unload_timeout_options: &[u32],
) {
    let options = list_available_model_options(catalog);
    let labels: Vec<slint::SharedString> =
        options.iter().map(|o| o.label.as_str().into()).collect();
    window.set_settings_model_labels(std::rc::Rc::new(slint::VecModel::from(labels)).into());

    let selected_label = options
        .iter()
        .find(|o| {
            o.engine_id == catalog.selected_engine_id && o.model_id == catalog.selected_model_id
        })
        .map(|o| o.label.clone())
        .unwrap_or_default();
    window.set_settings_selected_model_label(selected_label.into());

    let timeout_labels: Vec<slint::SharedString> = unload_timeout_options
        .iter()
        .map(|v| unload_timeout_label(*v).into())
        .collect();
    window.set_settings_unload_timeout_labels(
        std::rc::Rc::new(slint::VecModel::from(timeout_labels)).into(),
    );
    window.set_settings_unload_timeout_label(
        unload_timeout_label(model_unload_timeout_minutes).into(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unload_timeout_label_round_trips() {
        for value in [0, 5, 15, 60] {
            let label = unload_timeout_label(value);
            assert_eq!(
                parse_unload_timeout_label(&label),
                Some(value),
                "label={label}"
            );
        }
    }

    #[test]
    fn unload_timeout_zero_is_never_not_zero_minutes() {
        assert_eq!(unload_timeout_label(0), "Jamais");
    }

    #[test]
    fn phase_label_covers_every_phase() {
        assert_eq!(phase_label(TranscriptionRuntimePhase::Ready), "Prêt");
        assert_eq!(
            phase_label(TranscriptionRuntimePhase::DownloadRequired),
            "Téléchargement requis"
        );
        assert_eq!(
            phase_label(TranscriptionRuntimePhase::LoadRequired),
            "Chargement requis"
        );
    }
}
