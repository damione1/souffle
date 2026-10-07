//! Transcription model tab (SOU-188 milestone 6, AC2). The download/switch
//! state machine itself is never reimplemented here: every transition goes
//! through the same commands the Svelte controller calls
//! (`get_model_status`, `download_model`, `load_model`), this module only
//! builds the picker's flat option list and formats display strings - port
//! of `features/transcription/catalog.ts`'s pure functions.

use crate::MainWindow;
use slint::ComponentHandle;
use souffle_lib::engine::{
    TranscriptionCatalog, TranscriptionModelDescriptor, TranscriptionProfileSelection,
    TranscriptionRuntimeBackendDescriptor, TranscriptionRuntimePhase,
};
use souffle_lib::models::DownloadProgress;

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
                        label: if engine.id == souffle_lib::engine::APPLE_SPEECH_ENGINE_ID {
                            "__APPLE_SPEECH_SYSTEM__".into()
                        } else {
                            format!("{} \u{2014} {}", engine.label, model.label)
                        },
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
        .map(|m| {
            if engine_id == souffle_lib::engine::APPLE_SPEECH_ENGINE_ID {
                "__APPLE_SPEECH_SYSTEM__".into()
            } else {
                m.label.clone()
            }
        })
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
    progress.is_globally_complete()
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
        "__NEVER__".to_string()
    } else {
        crate::audio_ui::minute_label(value)
    }
}

pub fn parse_unload_timeout_label(label: &str) -> Option<u32> {
    if label == "__NEVER__" {
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
    let speech_model = catalog
        .engines
        .iter()
        .find(|engine| engine.id == souffle_lib::engine::APPLE_SPEECH_ENGINE_ID)
        .and_then(|engine| engine.models.first());
    let unavailable_reason = speech_model.and_then(|model| model.unavailable_reason);
    let labels = window.global::<crate::TranscriptionLabels>();
    if let Some(model) = speech_model {
        let code = model
            .supported_languages
            .first()
            .map(|s| s.replace('_', "-"))
            .unwrap_or_default();
        labels.set_speech_locale_code(code.as_str().into());
        labels.set_speech_name_en(
            model
                .localized_labels
                .get("en")
                .map_or(code.as_str(), String::as_str)
                .into(),
        );
        labels.set_speech_name_fr(
            model
                .localized_labels
                .get("fr")
                .map_or(code.as_str(), String::as_str)
                .into(),
        );
    }
    window.set_settings_apple_speech_unavailable(unavailable_reason.is_some());
    if let Some(reason) = unavailable_reason {
        window.set_settings_apple_speech_unavailable_reason(reason.into());
    }
    window.set_settings_apple_speech_selected(
        catalog.selected_engine_id == souffle_lib::engine::APPLE_SPEECH_ENGINE_ID,
    );
    let files_deletable = catalog
        .engines
        .iter()
        .find(|e| e.id == catalog.selected_engine_id)
        .and_then(|e| e.models.iter().find(|m| m.id == catalog.selected_model_id))
        .and_then(|m| {
            m.backends
                .iter()
                .find(|b| b.id == catalog.selected_backend_id)
        })
        .is_some_and(|backend| match &backend.assets {
            souffle_lib::engine::ModelAssetSource::Files { .. } => true,
            souffle_lib::engine::ModelAssetSource::SystemSpeech { .. } => false,
        });
    window.set_settings_model_files_deletable(files_deletable);
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

impl From<souffle_lib::engine::TranscriptionUnavailableReason>
    for crate::TranscriptionUnavailableReason
{
    fn from(reason: souffle_lib::engine::TranscriptionUnavailableReason) -> Self {
        use souffle_lib::engine::TranscriptionUnavailableReason as Domain;
        match reason {
            Domain::OsUnsupported => Self::OsUnsupported,
            Domain::DeviceUnsupported => Self::DeviceUnsupported,
            Domain::BuildUnsupported => Self::BuildUnsupported,
            Domain::LocaleUnsupported => Self::LocaleUnsupported,
            Domain::AssetsUnsupported => Self::AssetsUnsupported,
            Domain::CheckFailed => Self::CheckFailed,
        }
    }
}

impl From<crate::TranscriptionUnavailableReason>
    for souffle_lib::engine::TranscriptionUnavailableReason
{
    fn from(reason: crate::TranscriptionUnavailableReason) -> Self {
        match reason {
            crate::TranscriptionUnavailableReason::OsUnsupported => Self::OsUnsupported,
            crate::TranscriptionUnavailableReason::DeviceUnsupported => Self::DeviceUnsupported,
            crate::TranscriptionUnavailableReason::BuildUnsupported => Self::BuildUnsupported,
            crate::TranscriptionUnavailableReason::LocaleUnsupported => Self::LocaleUnsupported,
            crate::TranscriptionUnavailableReason::AssetsUnsupported => Self::AssetsUnsupported,
            crate::TranscriptionUnavailableReason::CheckFailed => Self::CheckFailed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use souffle_lib::models::DownloadStatus;

    #[test]
    fn speech_settings_project_locale_in_both_languages_and_preserve_timeout() {
        struct Platform;
        impl slint::platform::Platform for Platform {
            fn create_window_adapter(
                &self,
            ) -> Result<std::rc::Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError>
            {
                Ok(
                    slint::platform::software_renderer::MinimalSoftwareWindow::new(
                        Default::default(),
                    ),
                )
            }
        }
        let _ = slint::platform::set_platform(Box::new(Platform));
        let window = MainWindow::new().unwrap();
        let mut catalog = souffle_lib::commands::transcription_catalog_from_settings(
            &souffle_lib::settings::AppSettings::default(),
        )
        .unwrap();
        let engine = catalog
            .engines
            .iter_mut()
            .find(|e| e.id == souffle_lib::engine::APPLE_SPEECH_ENGINE_ID)
            .unwrap();
        let model = &mut engine.models[0];
        // A deterministic supported fixture, independent of this test machine.
        model.available_in_app = true;
        model.unavailable_reason = None;
        model.supported_languages = vec!["en_US".into()];
        model.localized_labels = [
            ("en".into(), "English (United States)".into()),
            ("fr".into(), "anglais (États-Unis)".into()),
        ]
        .into();
        model.backends = vec![TranscriptionRuntimeBackendDescriptor {
            id: souffle_lib::engine::APPLE_SPEECH_BACKEND_ID.into(),
            label: "SpeechAnalyzer".into(),
            description: String::new(),
            recommended: true,
            available_in_app: true,
            availability_note: None,
            assets: souffle_lib::engine::ModelAssetSource::SystemSpeech {
                locale: "en_US".into(),
            },
        }];
        catalog.selected_engine_id = engine.id.clone();
        catalog.selected_model_id = model.id.clone();
        catalog.selected_backend_id = model.backends[0].id.clone();
        populate_options(&window, &catalog, 15, &[0, 5, 15, 60]);
        assert!(window.get_settings_apple_speech_selected());
        assert!(!window.get_settings_model_files_deletable());
        assert_eq!(window.get_settings_unload_timeout_label(), "15 min");
        assert_eq!(
            window.get_settings_selected_model_label(),
            "__APPLE_SPEECH_SYSTEM__"
        );
        let labels = window.global::<crate::TranscriptionLabels>();
        for (locale, expected) in [
            (crate::AppLocale::En, "English (United States)"),
            (crate::AppLocale::Fr, "anglais (États-Unis)"),
        ] {
            window.set_settings_locale(locale);
            slint::platform::update_timers_and_animations();
            assert!(labels.invoke_speech_label().contains(expected));
            assert!(labels.invoke_speech_label().contains("en-US"));
        }
        let model = &mut catalog
            .engines
            .iter_mut()
            .find(|e| e.id == catalog.selected_engine_id)
            .unwrap()
            .models[0];
        model.available_in_app = false;
        model.backends.clear();
        model.unavailable_reason =
            Some(souffle_lib::engine::TranscriptionUnavailableReason::LocaleUnsupported);
        populate_options(&window, &catalog, 15, &[0, 5, 15, 60]);
        assert!(window.get_settings_apple_speech_unavailable());
        assert_eq!(
            window.get_settings_apple_speech_unavailable_reason(),
            crate::TranscriptionUnavailableReason::LocaleUnsupported
        );
        assert!(
            !list_available_model_options(&catalog)
                .iter()
                .any(|o| o.engine_id == catalog.selected_engine_id)
        );
        assert_eq!(window.get_settings_unload_timeout_label(), "15 min");
    }

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
        assert_eq!(unload_timeout_label(0), "__NEVER__");
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
