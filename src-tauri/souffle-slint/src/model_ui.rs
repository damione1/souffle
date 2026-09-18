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
use souffle_lib::models::{DownloadProgress, DownloadStatus};

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

/// Port of `findTranscriptionModel(...)?.label` - the bare model label
/// ("STT 1B FR/EN"), not `list_available_model_options`'s "Engine — Model"
/// (that combined form is for the settings picker, which needs to
/// disambiguate engines; `App.svelte`'s `StatusChip` reads this instead).
pub fn model_short_label(
    catalog: &TranscriptionCatalog,
    engine_id: &str,
    model_id: &str,
) -> String {
    catalog
        .engines
        .iter()
        .find(|engine| engine.id == engine_id)
        .and_then(|engine| engine.models.iter().find(|m| m.id == model_id))
        .map(|m| m.label.clone())
        .unwrap_or_default()
}

pub fn selected_model_short_label(catalog: &TranscriptionCatalog) -> String {
    model_short_label(
        catalog,
        &catalog.selected_engine_id,
        &catalog.selected_model_id,
    )
}

/// The catalogue is the source of truth for the active open-set identifiers.
/// Recording startup must use this selection rather than the library default:
/// users can legitimately select a different downloaded model in Settings.
pub fn selected_profile(catalog: &TranscriptionCatalog) -> TranscriptionProfileSelection {
    TranscriptionProfileSelection {
        engine_id: catalog.selected_engine_id.clone(),
        model_id: catalog.selected_model_id.clone(),
        backend_id: catalog.selected_backend_id.clone(),
    }
}

pub fn download_is_globally_complete(progress: &DownloadProgress) -> bool {
    matches!(progress.status, DownloadStatus::Complete)
        && progress.total_files > 0
        && progress.completed_files >= progress.total_files
}

pub fn phase_label(phase: TranscriptionRuntimePhase) -> &'static str {
    match phase {
        TranscriptionRuntimePhase::DownloadRequired => "Téléchargement requis",
        TranscriptionRuntimePhase::Downloading => "Téléchargement…",
        TranscriptionRuntimePhase::LoadRequired => "Chargement requis",
        TranscriptionRuntimePhase::Loading => "Chargement…",
        TranscriptionRuntimePhase::Ready => "Prêt",
        TranscriptionRuntimePhase::Unloading => "Déchargement…",
        TranscriptionRuntimePhase::Failed => "Erreur du modèle",
    }
}

impl From<TranscriptionRuntimePhase> for crate::TranscriptionPhase {
    fn from(phase: TranscriptionRuntimePhase) -> Self {
        match phase {
            TranscriptionRuntimePhase::DownloadRequired => Self::DownloadRequired,
            TranscriptionRuntimePhase::Downloading => Self::Downloading,
            TranscriptionRuntimePhase::LoadRequired => Self::LoadRequired,
            TranscriptionRuntimePhase::Loading => Self::Loading,
            TranscriptionRuntimePhase::Ready => Self::Ready,
            TranscriptionRuntimePhase::Unloading => Self::Unloading,
            TranscriptionRuntimePhase::Failed => Self::Failed,
        }
    }
}

impl From<crate::TranscriptionPhase> for TranscriptionRuntimePhase {
    fn from(phase: crate::TranscriptionPhase) -> Self {
        match phase {
            crate::TranscriptionPhase::DownloadRequired => Self::DownloadRequired,
            crate::TranscriptionPhase::Downloading => Self::Downloading,
            crate::TranscriptionPhase::LoadRequired => Self::LoadRequired,
            crate::TranscriptionPhase::Loading => Self::Loading,
            crate::TranscriptionPhase::Ready => Self::Ready,
            crate::TranscriptionPhase::Unloading => Self::Unloading,
            crate::TranscriptionPhase::Failed => Self::Failed,
        }
    }
}

pub fn populate_runtime(window: &MainWindow, phase: TranscriptionRuntimePhase) {
    window.set_model_runtime_phase(phase.into());
    window.set_settings_model_status_text(phase_label(phase).into());
}

/// The FSM already arbitrates StartLoad atomically. A losing concurrent caller
/// ignores its error only when this snapshot proves Loading/Ready; subsequent
/// transition notifications refresh the UI without a second lock or busy state.
pub fn load_is_already_in_progress_or_ready(phase: TranscriptionRuntimePhase) -> bool {
    match phase {
        TranscriptionRuntimePhase::Loading | TranscriptionRuntimePhase::Ready => true,
        TranscriptionRuntimePhase::DownloadRequired
        | TranscriptionRuntimePhase::Downloading
        | TranscriptionRuntimePhase::LoadRequired
        | TranscriptionRuntimePhase::Unloading
        | TranscriptionRuntimePhase::Failed => false,
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

    #[test]
    fn runtime_phase_round_trips_without_losing_in_flight_states() {
        for phase in [
            TranscriptionRuntimePhase::DownloadRequired,
            TranscriptionRuntimePhase::Downloading,
            TranscriptionRuntimePhase::LoadRequired,
            TranscriptionRuntimePhase::Loading,
            TranscriptionRuntimePhase::Ready,
            TranscriptionRuntimePhase::Unloading,
            TranscriptionRuntimePhase::Failed,
        ] {
            assert_eq!(
                TranscriptionRuntimePhase::from(crate::TranscriptionPhase::from(phase)),
                phase
            );
            assert!(!phase_label(phase).is_empty());
        }
    }

    #[test]
    fn startup_selection_uses_persisted_catalog_ids_not_defaults() {
        let catalog = TranscriptionCatalog {
            engines: Vec::new(),
            selected_engine_id: "configured-engine".into(),
            selected_model_id: "configured-model".into(),
            selected_backend_id: "configured-backend".into(),
        };
        let selected = selected_profile(&catalog);
        assert_eq!(selected.engine_id, catalog.selected_engine_id);
        assert_eq!(selected.model_id, catalog.selected_model_id);
        assert_eq!(selected.backend_id, catalog.selected_backend_id);
    }

    #[test]
    fn concurrent_load_only_accepts_a_loading_or_ready_fsm() {
        assert!(load_is_already_in_progress_or_ready(
            TranscriptionRuntimePhase::Loading
        ));
        assert!(load_is_already_in_progress_or_ready(
            TranscriptionRuntimePhase::Ready
        ));
        for phase in [
            TranscriptionRuntimePhase::DownloadRequired,
            TranscriptionRuntimePhase::Downloading,
            TranscriptionRuntimePhase::LoadRequired,
            TranscriptionRuntimePhase::Unloading,
            TranscriptionRuntimePhase::Failed,
        ] {
            assert!(!load_is_already_in_progress_or_ready(phase));
        }
    }

    #[test]
    fn download_only_completes_after_every_artifact() {
        let progress = |completed_files, total_files| DownloadProgress {
            file: "artifact".into(),
            downloaded_bytes: 0,
            total_bytes: None,
            completed_files,
            total_files,
            status: DownloadStatus::Complete,
        };

        assert!(!download_is_globally_complete(&progress(1, 0)));
        assert!(!download_is_globally_complete(&progress(1, 2)));
        assert!(download_is_globally_complete(&progress(2, 2)));
    }
}
