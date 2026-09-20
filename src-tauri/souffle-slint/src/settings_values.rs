//! Settings values are controlled by Rust. This module wires the value callbacks
//! and owns the save/read/publication round trip, without querying catalogues.

use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use slint::ComponentHandle;
use souffle_lib::audio::AudioInputDevice;
#[cfg(test)]
use souffle_lib::commands::SettingsEffectFailure;
use souffle_lib::commands::{SettingsSaveError, SettingsSaveLane, SettingsSaveOutcome};
use souffle_lib::settings::{AppSettings, SettingsOptions, Theme as SettingsTheme};
use souffle_lib::summary::SummaryProvidersStatus;

use crate::settings_io::{SettingsIoCoordinator, SettingsResponseOrder};
use crate::{MainWindow, Theme, audio_ui, data_ui, ia_ui, model_ui, settings_ui};

#[derive(Clone)]
pub(crate) struct SettingsCache {
    snapshot: Rc<RefCell<Option<AppSettings>>>,
    known: Rc<Cell<bool>>,
    window: slint::Weak<MainWindow>,
}

impl SettingsCache {
    pub(crate) fn new(window: &MainWindow) -> Self {
        Self {
            snapshot: Rc::new(RefCell::new(None)),
            known: Rc::new(Cell::new(false)),
            window: window.as_weak(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_observed(window: &MainWindow, settings: AppSettings) -> Self {
        Self {
            snapshot: Rc::new(RefCell::new(Some(settings))),
            known: Rc::new(Cell::new(true)),
            window: window.as_weak(),
        }
    }

    pub(crate) fn borrow(&self) -> Ref<'_, Option<AppSettings>> {
        self.snapshot.borrow()
    }

    pub(crate) fn replace_observed(&self, settings: AppSettings) {
        *self.snapshot.borrow_mut() = Some(settings);
        self.known.set(true);
    }

    pub(crate) fn known_snapshot(&self) -> Option<AppSettings> {
        self.known
            .get()
            .then(|| self.snapshot.borrow().clone())
            .flatten()
    }

    /// Last snapshot actually observed from persistence, even when a newer
    /// save could not be re-read and therefore made its freshness unknown.
    /// UI rollback must use this canonical value, never another optimistic
    /// preview from an earlier request.
    fn observed_snapshot(&self) -> Option<AppSettings> {
        self.snapshot.borrow().clone()
    }

    fn mark_unknown(&self) {
        self.known.set(false);
    }

    pub(crate) fn observe_save_outcome(&self, outcome: &SettingsSaveOutcome) {
        self.observe_save_outcome_silent(outcome);
        self.publish_save_status(outcome);
    }

    pub(crate) fn observe_save_outcome_silent(&self, outcome: &SettingsSaveOutcome) {
        match outcome {
            SettingsSaveOutcome::Observed { settings, .. } => {
                self.replace_observed(settings.as_ref().clone());
            }
            SettingsSaveOutcome::Unavailable { .. } => self.mark_unknown(),
        }
    }

    pub(crate) fn publish_save_status(&self, outcome: &SettingsSaveOutcome) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let message = match outcome {
            SettingsSaveOutcome::Observed { result, .. } => match result {
                Ok(()) => String::new(),
                Err(error) => error.user_message(),
            },
            SettingsSaveOutcome::Unavailable { result, read_error } => match result {
                Ok(()) => format!("Stored settings are unknown: {read_error}"),
                Err(error) => format!(
                    "{}. Stored settings are unknown: {read_error}",
                    error.user_message()
                ),
            },
        };
        window.set_settings_save_error(message.into());
    }

    #[cfg(test)]
    fn is_known(&self) -> bool {
        self.known.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsCommitStatus {
    Committed,
    NotCommitted,
}

pub(crate) fn save_outcome_commit_status(outcome: &SettingsSaveOutcome) -> SettingsCommitStatus {
    let result = match outcome {
        SettingsSaveOutcome::Observed { result, .. }
        | SettingsSaveOutcome::Unavailable { result, .. } => result,
    };
    match result {
        Ok(()) => SettingsCommitStatus::Committed,
        Err(SettingsSaveError::EffectFailedAfterCommit { .. }) => SettingsCommitStatus::Committed,
        Err(SettingsSaveError::Rejected { .. } | SettingsSaveError::NotCommitted { .. }) => {
            SettingsCommitStatus::NotCommitted
        }
    }
}

type DeviceCache = Rc<RefCell<Vec<AudioInputDevice>>>;
type SummaryCache = Rc<RefCell<Option<SummaryProvidersStatus>>>;

fn canonical_dark_after_save(
    theme: SettingsTheme,
    previous_theme: Option<SettingsTheme>,
    applied_dark: bool,
    resolve_system: impl FnOnce() -> bool,
) -> bool {
    match theme {
        SettingsTheme::Dark => true,
        SettingsTheme::Light => false,
        SettingsTheme::System if previous_theme == Some(SettingsTheme::System) => applied_dark,
        SettingsTheme::System => resolve_system(),
    }
}

/// Same round trip for immediate controls and the existing deferred callers.
/// The closures isolate OS side effects from controller tests, not a second store.
#[cfg(test)]
pub(crate) fn save_field(
    cache: &SettingsCache,
    load: impl FnOnce() -> Result<AppSettings, String>,
    save: impl FnOnce(AppSettings) -> SettingsSaveOutcome,
    mutate: impl FnOnce(&mut AppSettings),
) -> SettingsSaveOutcome {
    let cached = cache.borrow().clone();
    let original = match (cache.known.get(), cached) {
        (true, Some(settings)) => settings,
        (_, _) => match load() {
            Ok(settings) => settings,
            Err(read_error) => {
                cache.mark_unknown();
                let outcome = SettingsSaveOutcome::Unavailable {
                    result: Err(SettingsSaveError::NotCommitted {
                        message: "Settings could not be loaded before saving".into(),
                    }),
                    read_error,
                };
                cache.publish_save_status(&outcome);
                return outcome;
            }
        },
    };
    let mut candidate = original.clone();
    mutate(&mut candidate);
    let outcome = save(candidate);
    cache.observe_save_outcome(&outcome);
    outcome
}

pub(crate) struct SettingsValueController {
    window: slint::Weak<MainWindow>,
    cache: SettingsCache,
    io: Rc<SettingsIoCoordinator>,
    preview: RefCell<Option<AppSettings>>,
    devices: DeviceCache,
    summary: SummaryCache,
    apply_appearance: Rc<dyn Fn(bool)>,
}

impl SettingsValueController {
    pub(crate) fn new(
        window: &MainWindow,
        cache: SettingsCache,
        io: Rc<SettingsIoCoordinator>,
        devices: DeviceCache,
        summary: SummaryCache,
    ) -> Rc<Self> {
        Rc::new(Self {
            window: window.as_weak(),
            cache,
            io,
            preview: RefCell::new(None),
            devices,
            summary,
            apply_appearance: Rc::new(souffle_lib::native::appearance::apply_resolved),
        })
    }

    pub(crate) fn observe_loaded(&self, settings: &AppSettings) {
        *self.preview.borrow_mut() = Some(settings.clone());
    }

    fn apply(
        self: &Rc<Self>,
        lane: SettingsSaveLane,
        mutate: impl Fn(&mut AppSettings) + Send + Sync + 'static,
    ) {
        let mutation = Arc::new(mutate);
        let previous_theme = self
            .preview
            .borrow()
            .as_ref()
            .map(|settings| settings.theme)
            .or_else(|| self.cache.known_snapshot().map(|settings| settings.theme));
        let candidate = self
            .preview
            .borrow()
            .clone()
            .or_else(|| self.cache.known_snapshot());
        if let Some(mut candidate) = candidate {
            mutation(&mut candidate);
            *self.preview.borrow_mut() = Some(candidate.clone());
            if let Some(window) = self.window.upgrade() {
                self.project_snapshot(&window, &candidate, previous_theme);
            }
        }

        let worker_mutation = Arc::clone(&mutation);
        self.io
            .submit(lane, move |settings| worker_mutation(settings), |_, _| {});
    }

    fn publish_save_outcome(&self, order: SettingsResponseOrder, outcome: &SettingsSaveOutcome) {
        match order {
            SettingsResponseOrder::LatestVisible | SettingsResponseOrder::LatestHidden => {
                let settings = match outcome {
                    SettingsSaveOutcome::Observed { settings, .. } => {
                        Some(settings.as_ref().clone())
                    }
                    SettingsSaveOutcome::Unavailable { .. } => self.cache.observed_snapshot(),
                };
                if let Some(settings) = settings {
                    let previous_theme =
                        self.preview.borrow().as_ref().map(|preview| preview.theme);
                    *self.preview.borrow_mut() = Some(settings.clone());
                    if let Some(window) = self.window.upgrade() {
                        // These helpers project scalar/dropdown values only;
                        // prompt/template/list drafts remain owned by the
                        // draft controller and are never overwritten here.
                        settings_ui::populate(&window, &settings);
                        data_ui::populate(&window, &settings);
                        self.project_snapshot(&window, &settings, previous_theme);
                    }
                }
            }
            SettingsResponseOrder::Intermediate | SettingsResponseOrder::Stale => {}
        }
    }

    fn project_snapshot(
        &self,
        window: &MainWindow,
        settings: &AppSettings,
        previous_theme: Option<SettingsTheme>,
    ) {
        let applied_dark = window.global::<Theme>().get_dark();
        let dark = canonical_dark_after_save(settings.theme, previous_theme, applied_dark, || {
            settings_ui::resolve_dark(SettingsTheme::System)
        });
        self.project(window, settings);
        // NSAppearance is AppKit-owned and stays on Slint's UI thread. The
        // worker runs only database/ServiceManagement/engine effects.
        if applied_dark != dark {
            window.global::<Theme>().set_dark(dark);
            (self.apply_appearance)(dark);
        }
    }

    /// Publish only the immediate value controls. Do not rebuild catalogues,
    /// refresh providers/devices, or overwrite prompt/URL/list editor drafts.
    fn project(&self, window: &MainWindow, settings: &AppSettings) {
        window.set_settings_theme(settings_ui::theme_to_slint(settings.theme));
        window.set_settings_locale(settings_ui::locale_to_slint(&settings.locale));
        window.set_settings_paste_method(settings_ui::paste_method_to_slint(settings.paste_method));
        window.set_settings_paste_delay_ms(settings.paste_delay_ms as i32);
        window.set_settings_feedback_sounds_volume(settings.feedback_sounds_volume as i32);
        window.set_settings_calendar_reminder_minutes(settings.calendar_reminder_minutes as i32);
        window.set_settings_clamshell_device_label(
            audio_ui::clamshell_device_label(
                &self.devices.borrow(),
                settings.clamshell_audio_device.as_deref(),
            )
            .into(),
        );
        window.set_settings_meeting_transcription_language(settings_ui::meeting_language_to_slint(
            settings.meeting_transcription_language,
        ));
        window.set_settings_meeting_autostop_label(
            audio_ui::minute_label(settings.meeting_autostop_minutes).into(),
        );
        window.set_settings_meeting_max_duration_label(
            audio_ui::minute_label(settings.meeting_max_duration_minutes).into(),
        );
        window.set_settings_meeting_audio_retention(settings_ui::audio_retention_to_slint(
            settings.meeting_audio_retention,
        ));
        let summary = self.summary.borrow();
        let models = summary
            .as_ref()
            .map(|status| status.models.as_slice())
            .unwrap_or_default();
        window.set_settings_selected_summary_model_label(
            ia_ui::selected_summary_model_label(models, &settings.ollama_model).into(),
        );
        window.set_settings_unload_timeout_label(
            model_ui::unload_timeout_label(settings.model_unload_timeout_minutes).into(),
        );
    }
}

pub(crate) fn wire(window: &MainWindow, controller: Rc<SettingsValueController>) {
    let weak_controller = Rc::downgrade(&controller);
    controller.io.set_save_projection(move |order, outcome| {
        if let Some(controller) = weak_controller.upgrade() {
            controller.publish_save_outcome(order, outcome);
        }
    });
    let c = controller.clone();
    window.on_settings_theme_changed(move |value| {
        crate::settings_instrumentation::handler(
            crate::settings_instrumentation::SettingsScenario::Theme,
        );
        c.apply(SettingsSaveLane::General, move |s| {
            s.theme = settings_ui::theme_from_slint(value)
        });
    });
    // The header must obey the same durable-value contract as the Settings buttons.
    let c = controller.clone();
    window.on_theme_toggle_requested(move || {
        let Some(window) = c.window.upgrade() else {
            return;
        };
        let theme = if window.global::<Theme>().get_dark() {
            souffle_lib::settings::Theme::Light
        } else {
            souffle_lib::settings::Theme::Dark
        };
        c.apply(SettingsSaveLane::General, move |s| s.theme = theme);
    });
    let c = controller.clone();
    window.on_settings_locale_changed(move |value| {
        crate::select_app_locale(value);
        c.apply(SettingsSaveLane::General, move |s| {
            s.locale = settings_ui::locale_from_slint(value).into()
        });
    });
    let c = controller.clone();
    window.on_settings_paste_method_changed(move |value| {
        c.apply(SettingsSaveLane::General, move |s| {
            s.paste_method = settings_ui::paste_method_from_slint(value)
        });
    });
    let c = controller.clone();
    window.on_settings_paste_delay_changed(move |value| {
        crate::settings_instrumentation::handler(
            crate::settings_instrumentation::SettingsScenario::Number,
        );
        c.apply(SettingsSaveLane::General, move |s| {
            s.paste_delay_ms = value.max(0) as u64
        });
    });
    let c = controller.clone();
    let options = SettingsOptions::current();
    let volume_min = options.feedback_sounds_volume_min as i32;
    let volume_max = options.feedback_sounds_volume_max as i32;
    window.on_settings_feedback_sounds_volume_changed(move |value| {
        c.apply(SettingsSaveLane::General, move |s| {
            s.feedback_sounds_volume = value.clamp(volume_min, volume_max) as u32
        });
    });
    let c = controller.clone();
    let reminder_min = options.calendar_reminder_minutes_min as i32;
    let reminder_max = options.calendar_reminder_minutes_max as i32;
    window.on_settings_calendar_reminder_minutes_changed(move |value| {
        c.apply(SettingsSaveLane::General, move |s| {
            s.calendar_reminder_minutes = value.clamp(reminder_min, reminder_max) as u32
        });
    });
    let c = controller.clone();
    window.on_settings_clamshell_device_changed(move |label| {
        let uid = audio_ui::resolve_device_uid(&c.devices.borrow(), &label);
        c.apply(SettingsSaveLane::General, move |s| {
            s.clamshell_audio_device = uid.clone()
        });
    });
    let c = controller.clone();
    window.on_settings_meeting_transcription_language_changed(move |value| {
        c.apply(SettingsSaveLane::General, move |s| {
            s.meeting_transcription_language = settings_ui::meeting_language_from_slint(value)
        });
    });
    let c = controller.clone();
    window.on_settings_meeting_autostop_changed(move |label| {
        if let Some(minutes) = audio_ui::parse_minute_label(&label) {
            c.apply(SettingsSaveLane::General, move |s| {
                s.meeting_autostop_minutes = minutes
            });
        }
    });
    let c = controller.clone();
    window.on_settings_meeting_max_duration_changed(move |label| {
        if let Some(minutes) = audio_ui::parse_minute_label(&label) {
            c.apply(SettingsSaveLane::General, move |s| {
                s.meeting_max_duration_minutes = minutes
            });
        }
    });
    let c = controller.clone();
    window.on_settings_meeting_audio_retention_changed(move |value| {
        c.apply(SettingsSaveLane::General, move |s| {
            s.meeting_audio_retention = settings_ui::audio_retention_from_slint(value)
        });
    });
    let c = controller.clone();
    window.on_settings_ollama_model_changed(move |id| {
        let id = id.to_string();
        let is_known_ollama_model = c
            .summary
            .borrow()
            .as_ref()
            .is_some_and(|status| ia_ui::contains_summary_model_id(&status.models, &id));
        if is_known_ollama_model {
            c.apply(SettingsSaveLane::General, move |s| {
                s.ollama_model = id.clone()
            });
        }
    });
    let c = controller;
    window.on_settings_unload_timeout_changed(move |label| {
        if let Some(minutes) = model_ui::parse_unload_timeout_label(&label) {
            c.apply(SettingsSaveLane::General, move |s| {
                s.model_unload_timeout_minutes = minutes
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::{Platform, WindowAdapter, software_renderer::MinimalSoftwareWindow};
    use souffle_lib::audio::TransportType;
    use souffle_lib::db::Database;
    use souffle_lib::settings::{MeetingAudioRetention, MeetingTranscriptionLanguage, PasteMethod};
    use souffle_lib::summary::{SummaryModelDescriptor, SummaryProviderKind};
    use std::cell::Cell;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    struct TestPlatform;

    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(Default::default()))
        }
    }

    fn test_window() -> MainWindow {
        let _ = slint::platform::set_platform(Box::new(TestPlatform));
        MainWindow::new().unwrap()
    }

    fn committed_result(result: Result<(), String>) -> Result<(), SettingsSaveError> {
        result.map_err(|message| SettingsSaveError::NotCommitted { message })
    }

    fn drain_until(io: &SettingsIoCoordinator, counter: &AtomicUsize, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            io.drain_for_test();
            if counter.load(Ordering::SeqCst) >= expected && io.is_idle_for_test() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for settings response"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn device(uid: &str, name: &str) -> AudioInputDevice {
        AudioInputDevice {
            uid: uid.into(),
            name: name.into(),
            transport: TransportType::Usb,
            is_default: false,
        }
    }

    fn summary_status() -> SummaryProvidersStatus {
        SummaryProvidersStatus {
            ollama_url: "http://127.0.0.1:11434".into(),
            ollama_available: true,
            apple_intelligence_available: false,
            apple_intelligence_is_stub: false,
            apple_intelligence_unavailable_reason: Some(
                souffle_lib::apple_intelligence::AppleIntelligenceUnavailableReason::Unknown,
            ),
            recommended_ollama_model: "model-b".into(),
            models: vec![
                SummaryModelDescriptor {
                    id: "model-a".into(),
                    label: "Model A".into(),
                    provider: SummaryProviderKind::Ollama,
                    can_summarize: true,
                },
                SummaryModelDescriptor {
                    id: "model-b".into(),
                    label: "Model B".into(),
                    provider: SummaryProviderKind::Ollama,
                    can_summarize: true,
                },
            ],
        }
    }

    struct DatabaseHarness {
        window: MainWindow,
        db: Rc<Database>,
        io: Rc<SettingsIoCoordinator>,
        cache: SettingsCache,
        devices: DeviceCache,
        summary: SummaryCache,
        fallback_loads: Arc<AtomicUsize>,
        saves: Arc<AtomicUsize>,
        appearance: Rc<Cell<Option<bool>>>,
        _directory: tempfile::TempDir,
    }

    impl DatabaseHarness {
        fn new(initial: AppSettings) -> Self {
            Self::with_settings_population(initial, true)
        }

        fn new_unopened(initial: AppSettings) -> Self {
            Self::with_settings_population(initial, false)
        }

        fn with_settings_population(initial: AppSettings, populate_settings: bool) -> Self {
            let window = test_window();
            let directory = tempfile::tempdir().unwrap();
            let db = Rc::new(Database::open(&directory.path().join("settings.db")).unwrap());
            initial.save(&db).unwrap();
            let initial = AppSettings::load(&db).unwrap();
            let devices = Rc::new(RefCell::new(vec![device("mic-1", "Studio Mic")]));
            let summary = Rc::new(RefCell::new(Some(summary_status())));

            if populate_settings {
                settings_ui::populate(&window, &initial);
            }
            audio_ui::populate_device_pickers(
                &window,
                &devices.borrow(),
                initial.audio_device.as_deref().unwrap_or_default(),
                initial.clamshell_audio_device.as_deref(),
            );
            ia_ui::populate_intelligence(&window, &initial, summary.borrow().as_ref().unwrap());
            window
                .global::<Theme>()
                .set_dark(settings_ui::resolve_dark(initial.theme));

            let cache = SettingsCache::with_observed(&window, initial.clone());
            let fallback_loads = Arc::new(AtomicUsize::new(0));
            let saves = Arc::new(AtomicUsize::new(0));
            let database_path = directory.path().join("settings.db");
            let load_path = database_path.clone();
            let load_count = Arc::clone(&fallback_loads);
            let save_path = database_path.clone();
            let save_count = Arc::clone(&saves);
            let load = move || {
                load_count.fetch_add(1, Ordering::SeqCst);
                let database = Database::open(&load_path)?;
                AppSettings::load(&database)
            };
            let save = move |settings: AppSettings| {
                save_count.fetch_add(1, Ordering::SeqCst);
                let database = Database::open(&save_path).unwrap();
                let result = committed_result(settings.save(&database));
                SettingsSaveOutcome::from_results(result, AppSettings::load(&database))
            };
            let io = SettingsIoCoordinator::with_functions(
                cache.clone(),
                load,
                || Ok(AppSettings::default()),
                save,
                |settings| SettingsSaveOutcome::Observed {
                    settings: Box::new(settings),
                    result: Ok(()),
                },
            );
            let token = io.begin_open();
            assert!(io.seed_if_current(token, io.current_revision(), initial.clone()));
            let appearance = Rc::new(Cell::new(None));
            let projected_appearance = appearance.clone();
            let controller = Rc::new(SettingsValueController {
                window: window.as_weak(),
                cache: cache.clone(),
                io: io.clone(),
                preview: RefCell::new(Some(initial)),
                devices: devices.clone(),
                summary: summary.clone(),
                apply_appearance: Rc::new(move |dark| projected_appearance.set(Some(dark))),
            });
            wire(&window, controller);

            Self {
                window,
                db,
                io,
                cache,
                devices,
                summary,
                fallback_loads,
                saves,
                appearance,
                _directory: directory,
            }
        }

        fn wait_for_saves(&self, expected: usize) {
            drain_until(&self.io, &self.saves, expected);
        }
    }

    #[test]
    fn unchanged_system_theme_reuses_the_applied_value_without_native_resolution() {
        let resolutions = Cell::new(0);
        let dark = canonical_dark_after_save(
            SettingsTheme::System,
            Some(SettingsTheme::System),
            false,
            || {
                resolutions.set(resolutions.get() + 1);
                true
            },
        );
        assert!(!dark);
        assert_eq!(resolutions.get(), 0);

        let dark = canonical_dark_after_save(
            SettingsTheme::System,
            Some(SettingsTheme::Light),
            false,
            || {
                resolutions.set(resolutions.get() + 1);
                true
            },
        );
        assert!(dark);
        assert_eq!(resolutions.get(), 1);
    }

    #[test]
    fn volume_and_reminder_callbacks_clamp_to_backend_options() {
        let harness = DatabaseHarness::new(AppSettings::default());
        let options = SettingsOptions::current();

        harness
            .window
            .invoke_settings_feedback_sounds_volume_changed(-1);
        harness
            .window
            .invoke_settings_calendar_reminder_minutes_changed(0);
        harness.wait_for_saves(2);
        let lower = AppSettings::load(&harness.db).unwrap();
        assert_eq!(
            lower.feedback_sounds_volume,
            options.feedback_sounds_volume_min
        );
        assert_eq!(
            lower.calendar_reminder_minutes,
            options.calendar_reminder_minutes_min
        );

        harness
            .window
            .invoke_settings_feedback_sounds_volume_changed(i32::MAX);
        harness
            .window
            .invoke_settings_calendar_reminder_minutes_changed(i32::MAX);
        harness.wait_for_saves(4);
        let upper = AppSettings::load(&harness.db).unwrap();
        assert_eq!(
            upper.feedback_sounds_volume,
            options.feedback_sounds_volume_max
        );
        assert_eq!(
            upper.calendar_reminder_minutes,
            options.calendar_reminder_minutes_max
        );
    }

    fn assert_two_rapid_saves_use_last_observed_snapshot(first_commits: bool, expected_delay: i32) {
        let window = test_window();
        let initial = AppSettings {
            paste_delay_ms: 150,
            ..AppSettings::default()
        };
        settings_ui::populate(&window, &initial);
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let step = Arc::new(AtomicUsize::new(0));
        let save_step = Arc::clone(&step);
        let initial_for_save = initial.clone();
        let save = move |candidate: AppSettings| match save_step.fetch_add(1, Ordering::SeqCst) {
            0 if first_commits => SettingsSaveOutcome::Observed {
                settings: Box::new(candidate),
                result: Ok(()),
            },
            0 => SettingsSaveOutcome::Observed {
                settings: Box::new(initial_for_save.clone()),
                result: Err(SettingsSaveError::Rejected {
                    message: "first write rejected".into(),
                }),
            },
            1 => SettingsSaveOutcome::Unavailable {
                result: Err(SettingsSaveError::NotCommitted {
                    message: "second write rejected".into(),
                }),
                read_error: "injected reread failure".into(),
            },
            _ => unreachable!(),
        };
        let load_initial = initial.clone();
        let io = SettingsIoCoordinator::with_functions(
            cache.clone(),
            move || Ok(load_initial.clone()),
            || Ok(AppSettings::default()),
            save,
            |settings| SettingsSaveOutcome::Observed {
                settings: Box::new(settings),
                result: Ok(()),
            },
        );
        let token = io.begin_open();
        assert!(io.seed_if_current(token, 0, initial.clone()));
        let controller = Rc::new(SettingsValueController {
            window: window.as_weak(),
            cache,
            io: io.clone(),
            preview: RefCell::new(Some(initial)),
            devices: Rc::new(RefCell::new(Vec::new())),
            summary: Rc::new(RefCell::new(None)),
            apply_appearance: Rc::new(|_| {}),
        });
        wire(&window, controller);

        window.invoke_settings_paste_delay_changed(200);
        window.invoke_settings_paste_delay_changed(250);
        drain_until(&io, &step, 2);

        assert_eq!(window.get_settings_paste_delay_ms(), expected_delay);
    }

    #[test]
    fn unavailable_latest_response_does_not_revive_an_earlier_rejected_preview() {
        assert_two_rapid_saves_use_last_observed_snapshot(false, 150);
    }

    #[test]
    fn unavailable_latest_response_keeps_the_previous_committed_snapshot() {
        assert_two_rapid_saves_use_last_observed_snapshot(true, 200);
    }

    #[test]
    fn rejected_header_theme_toggle_rolls_back_while_settings_are_closed() {
        let window = test_window();
        let initial = AppSettings {
            theme: SettingsTheme::Light,
            ..AppSettings::default()
        };
        window.global::<Theme>().set_dark(false);
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let saves = Arc::new(AtomicUsize::new(0));
        let save_count = Arc::clone(&saves);
        let rejected_snapshot = initial.clone();
        let load_initial = initial.clone();
        let io = SettingsIoCoordinator::with_functions(
            cache.clone(),
            move || Ok(load_initial.clone()),
            || Ok(AppSettings::default()),
            move |_| {
                save_count.fetch_add(1, Ordering::SeqCst);
                SettingsSaveOutcome::Observed {
                    settings: Box::new(rejected_snapshot.clone()),
                    result: Err(SettingsSaveError::Rejected {
                        message: "theme rejected".into(),
                    }),
                }
            },
            |settings| SettingsSaveOutcome::Observed {
                settings: Box::new(settings),
                result: Ok(()),
            },
        );
        let appearance = Rc::new(Cell::new(None));
        let projected_appearance = appearance.clone();
        let controller = Rc::new(SettingsValueController {
            window: window.as_weak(),
            cache,
            io: io.clone(),
            preview: RefCell::new(Some(initial)),
            devices: Rc::new(RefCell::new(Vec::new())),
            summary: Rc::new(RefCell::new(None)),
            apply_appearance: Rc::new(move |dark| projected_appearance.set(Some(dark))),
        });
        wire(&window, controller);

        window.invoke_theme_toggle_requested();
        assert!(window.global::<Theme>().get_dark());
        drain_until(&io, &saves, 1);

        assert!(!window.global::<Theme>().get_dark());
        assert_eq!(window.get_settings_theme(), crate::AppTheme::Light);
        assert_eq!(appearance.get(), Some(false));
        assert!(window.get_settings_save_error().contains("theme rejected"));
    }

    #[test]
    fn later_generic_save_projects_the_canonical_theme_after_an_intermediate_rejection() {
        let window = test_window();
        let initial = AppSettings {
            theme: SettingsTheme::Light,
            auto_paste: true,
            ..AppSettings::default()
        };
        settings_ui::populate(&window, &initial);
        window.global::<Theme>().set_dark(false);
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let saves = Arc::new(AtomicUsize::new(0));
        let save_count = Arc::clone(&saves);
        let rejected = initial.clone();
        let io = SettingsIoCoordinator::with_functions(
            cache.clone(),
            {
                let initial = initial.clone();
                move || Ok(initial.clone())
            },
            {
                let initial = initial.clone();
                move || Ok(initial.clone())
            },
            move |candidate| match save_count.fetch_add(1, Ordering::SeqCst) {
                0 => SettingsSaveOutcome::Observed {
                    settings: Box::new(rejected.clone()),
                    result: Err(SettingsSaveError::Rejected {
                        message: "theme rejected".into(),
                    }),
                },
                1 => SettingsSaveOutcome::Observed {
                    settings: Box::new(candidate),
                    result: Ok(()),
                },
                _ => unreachable!(),
            },
            |settings| SettingsSaveOutcome::Observed {
                settings: Box::new(settings),
                result: Ok(()),
            },
        );
        io.begin_open();
        let appearance = Rc::new(Cell::new(None));
        let projected_appearance = appearance.clone();
        let controller = Rc::new(SettingsValueController {
            window: window.as_weak(),
            cache,
            io: io.clone(),
            preview: RefCell::new(Some(initial)),
            devices: Rc::new(RefCell::new(Vec::new())),
            summary: Rc::new(RefCell::new(None)),
            apply_appearance: Rc::new(move |dark| projected_appearance.set(Some(dark))),
        });
        wire(&window, controller);

        window.invoke_theme_toggle_requested();
        assert!(window.global::<Theme>().get_dark());
        io.submit(
            SettingsSaveLane::General,
            |settings| settings.auto_paste = false,
            |_, _| {},
        );
        drain_until(&io, &saves, 2);

        assert!(!window.global::<Theme>().get_dark());
        assert_eq!(window.get_settings_theme(), crate::AppTheme::Light);
        assert!(!window.get_settings_auto_paste());
        assert_eq!(appearance.get(), Some(false));
    }

    #[test]
    fn rejected_legacy_autostart_save_rolls_back_the_optimistic_control() {
        let window = test_window();
        let initial = AppSettings {
            autostart_enabled: false,
            ..AppSettings::default()
        };
        settings_ui::populate(&window, &initial);
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let saves = Arc::new(AtomicUsize::new(0));
        let save_count = Arc::clone(&saves);
        let rejected = initial.clone();
        let io = SettingsIoCoordinator::with_functions(
            cache.clone(),
            {
                let initial = initial.clone();
                move || Ok(initial.clone())
            },
            {
                let initial = initial.clone();
                move || Ok(initial.clone())
            },
            |settings| SettingsSaveOutcome::Observed {
                settings: Box::new(settings),
                result: Ok(()),
            },
            move |_| {
                save_count.fetch_add(1, Ordering::SeqCst);
                SettingsSaveOutcome::Observed {
                    settings: Box::new(rejected.clone()),
                    result: Err(SettingsSaveError::Rejected {
                        message: "autostart rejected".into(),
                    }),
                }
            },
        );
        io.begin_open();
        let controller = Rc::new(SettingsValueController {
            window: window.as_weak(),
            cache,
            io: io.clone(),
            preview: RefCell::new(Some(initial)),
            devices: Rc::new(RefCell::new(Vec::new())),
            summary: Rc::new(RefCell::new(None)),
            apply_appearance: Rc::new(|_| {}),
        });
        wire(&window, controller);

        window.set_settings_autostart_enabled(true);
        io.submit(
            SettingsSaveLane::Autostart,
            |settings| settings.autostart_enabled = true,
            |_, _| {},
        );
        drain_until(&io, &saves, 1);

        assert!(!window.get_settings_autostart_enabled());
        assert!(
            window
                .get_settings_save_error()
                .contains("autostart rejected")
        );
    }

    #[test]
    fn all_thirteen_callbacks_publish_the_persisted_snapshot_without_reloading_catalogues() {
        {
            let harness = DatabaseHarness::new_unopened(AppSettings {
                theme: SettingsTheme::Light,
                ..AppSettings::default()
            });
            let window = &harness.window;

            // Settings has not populated its enum yet, but startup has already
            // applied the persisted light theme to the global palette.
            assert_eq!(window.get_settings_theme(), crate::AppTheme::Dark);
            assert!(!window.global::<Theme>().get_dark());
            assert_eq!(harness.appearance.get(), None);

            window.invoke_theme_toggle_requested();
            harness.wait_for_saves(1);

            assert_eq!(window.get_settings_theme(), crate::AppTheme::Dark);
            assert!(window.global::<Theme>().get_dark());
            assert_eq!(harness.appearance.get(), Some(true));
            assert_eq!(
                AppSettings::load(&harness.db).unwrap().theme,
                SettingsTheme::Dark
            );
        }

        let initial = AppSettings {
            paste_delay_ms: 150,
            ollama_model: "model-a".into(),
            ..AppSettings::default()
        };
        let harness = DatabaseHarness::new(initial.clone());
        let window = &harness.window;
        let drafts = (
            initial.dictation_polish_templates.clone(),
            initial.summary_templates.clone(),
        );

        window.invoke_settings_theme_changed(crate::AppTheme::Dark);
        window.invoke_settings_locale_changed(crate::AppLocale::Fr);
        window.invoke_settings_paste_method_changed(crate::PasteMethod::Typing);
        window.invoke_settings_paste_delay_changed(window.get_settings_paste_delay_ms() + 50);
        assert_eq!(window.get_settings_paste_delay_ms(), 200);
        window.invoke_settings_paste_delay_changed(window.get_settings_paste_delay_ms() + 50);
        window.invoke_settings_feedback_sounds_volume_changed(16);
        window.invoke_settings_calendar_reminder_minutes_changed(5);
        window.invoke_settings_clamshell_device_changed("Studio Mic".into());
        window.invoke_settings_meeting_transcription_language_changed(crate::MeetingLanguage::En);
        window.invoke_settings_meeting_autostop_changed("5 min".into());
        window.invoke_settings_meeting_max_duration_changed("2 h".into());
        window.invoke_settings_meeting_audio_retention_changed(crate::AudioRetention::Keep7d);
        window.invoke_settings_ollama_model_changed("model-b".into());
        window.invoke_settings_unload_timeout_changed("5 min".into());
        harness.wait_for_saves(14);

        assert_eq!(window.get_settings_theme(), crate::AppTheme::Dark);
        assert!(window.global::<Theme>().get_dark());
        assert_eq!(harness.appearance.get(), Some(true));
        assert_eq!(window.get_settings_locale(), crate::AppLocale::Fr);
        assert_eq!(
            window.get_settings_paste_method(),
            crate::PasteMethod::Typing
        );
        assert_eq!(window.get_settings_paste_delay_ms(), 250);
        assert_eq!(window.get_settings_feedback_sounds_volume(), 16);
        assert_eq!(window.get_settings_calendar_reminder_minutes(), 5);
        assert_eq!(
            window.get_settings_clamshell_device_label().as_str(),
            "Studio Mic"
        );
        assert_eq!(
            window.get_settings_meeting_transcription_language(),
            crate::MeetingLanguage::En
        );
        assert_eq!(
            window.get_settings_meeting_autostop_label().as_str(),
            "5 min"
        );
        assert_eq!(
            window.get_settings_meeting_max_duration_label().as_str(),
            "2 h"
        );
        assert_eq!(
            window.get_settings_meeting_audio_retention(),
            crate::AudioRetention::Keep7d
        );
        assert_eq!(
            window.get_settings_selected_summary_model_label().as_str(),
            "Model B"
        );
        assert_eq!(window.get_settings_unload_timeout_label().as_str(), "5 min");
        assert!(window.get_settings_save_error().is_empty());

        let stored = AppSettings::load(&harness.db).unwrap();
        assert_eq!(stored.theme, SettingsTheme::Dark);
        assert_eq!(stored.locale, "fr");
        assert_eq!(stored.paste_method, PasteMethod::Type);
        assert_eq!(stored.paste_delay_ms, 250);
        assert_eq!(stored.feedback_sounds_volume, 16);
        assert_eq!(stored.calendar_reminder_minutes, 5);
        assert_eq!(stored.clamshell_audio_device.as_deref(), Some("mic-1"));
        assert_eq!(
            stored.meeting_transcription_language,
            MeetingTranscriptionLanguage::En
        );
        assert_eq!(stored.meeting_autostop_minutes, 5);
        assert_eq!(stored.meeting_max_duration_minutes, 120);
        assert_eq!(
            stored.meeting_audio_retention,
            MeetingAudioRetention::Keep7d
        );
        assert_eq!(stored.ollama_model, "model-b");
        assert_eq!(stored.model_unload_timeout_minutes, 5);

        assert_eq!(harness.saves.load(Ordering::SeqCst), 14);
        assert_eq!(harness.fallback_loads.load(Ordering::SeqCst), 0);
        assert_eq!(harness.devices.borrow()[0].uid, "mic-1");
        assert_eq!(
            harness.summary.borrow().as_ref().unwrap().models[1].id,
            "model-b"
        );
        {
            let cache = harness.cache.borrow();
            let cached = cache.as_ref().unwrap();
            assert_eq!(cached.dictation_polish_templates, drafts.0);
            assert_eq!(cached.summary_templates, drafts.1);
        }

        // The header follows the same canonical theme round trip.
        window.invoke_theme_toggle_requested();
        harness.wait_for_saves(15);
        assert_eq!(window.get_settings_theme(), crate::AppTheme::Light);
        assert!(!window.global::<Theme>().get_dark());
        assert_eq!(harness.appearance.get(), Some(false));
        assert_eq!(
            AppSettings::load(&harness.db).unwrap().theme,
            SettingsTheme::Light
        );

        // A value accepted by the callback but normalized by AppSettings must
        // publish the persisted default, never the requested label.
        window.invoke_settings_unload_timeout_changed("7 min".into());
        harness.wait_for_saves(16);
        assert_eq!(window.get_settings_unload_timeout_label().as_str(), "1 h");
        assert_eq!(
            AppSettings::load(&harness.db)
                .unwrap()
                .model_unload_timeout_minutes,
            60
        );
        assert_eq!(harness.saves.load(Ordering::SeqCst), 16);

        assert_failure_projection_contract();
    }

    #[test]
    fn ollama_model_callback_persists_the_selected_id_when_labels_match() {
        let initial = AppSettings {
            ollama_model: "model-a".into(),
            ..AppSettings::default()
        };
        let harness = DatabaseHarness::new(initial);
        harness.summary.borrow_mut().as_mut().unwrap().models[1].label = "Model A".into();

        harness
            .window
            .invoke_settings_ollama_model_changed("model-b".into());
        harness.wait_for_saves(1);

        assert_eq!(
            AppSettings::load(&harness.db).unwrap().ollama_model,
            "model-b"
        );
        assert_eq!(
            harness
                .window
                .get_settings_selected_summary_model_label()
                .as_str(),
            "Model A"
        );
    }

    fn assert_failure_projection_contract() {
        let window = test_window();
        let directory = tempfile::tempdir().unwrap();
        let db = Rc::new(Database::open(&directory.path().join("settings.db")).unwrap());
        let initial = AppSettings {
            paste_delay_ms: 150,
            ..AppSettings::default()
        };
        initial.save(&db).unwrap();
        settings_ui::populate(&window, &initial);

        let cache = SettingsCache::with_observed(&window, initial.clone());
        let step = Arc::new(AtomicUsize::new(0));
        let database_path = directory.path().join("settings.db");
        let load_path = database_path.clone();
        let save_path = database_path.clone();
        let save_step = Arc::clone(&step);
        let save = move |mut candidate: AppSettings| {
            let current_step = save_step.fetch_add(1, Ordering::SeqCst);
            let database = Database::open(&save_path).unwrap();
            match current_step {
                0 => SettingsSaveOutcome::Observed {
                    settings: Box::new(AppSettings::load(&database).unwrap()),
                    result: Err(SettingsSaveError::Rejected {
                        message: "write rejected".into(),
                    }),
                },
                1 => {
                    candidate.dictation_polish_templates[0].label =
                        "Committed polish template".into();
                    candidate.summary_templates[0].name = "Committed summary template".into();
                    candidate.save(&database).unwrap();
                    SettingsSaveOutcome::Observed {
                        settings: Box::new(AppSettings::load(&database).unwrap()),
                        result: Err(SettingsSaveError::EffectFailedAfterCommit {
                            cause: SettingsEffectFailure::Logging {
                                message: "native effect failed after commit".into(),
                            },
                        }),
                    }
                }
                2 => {
                    candidate.save(&database).unwrap();
                    SettingsSaveOutcome::Unavailable {
                        result: Ok(()),
                        read_error: "injected reread failure".into(),
                    }
                }
                3 => {
                    let result = committed_result(candidate.save(&database));
                    SettingsSaveOutcome::from_results(result, AppSettings::load(&database))
                }
                4 => {
                    candidate.save(&database).unwrap();
                    SettingsSaveOutcome::Unavailable {
                        result: Ok(()),
                        read_error: "second injected reread failure".into(),
                    }
                }
                _ => unreachable!(),
            }
        };
        let io = SettingsIoCoordinator::with_functions(
            cache.clone(),
            move || {
                let database = Database::open(&load_path)?;
                AppSettings::load(&database)
            },
            || Ok(AppSettings::default()),
            save,
            |settings| SettingsSaveOutcome::Observed {
                settings: Box::new(settings),
                result: Ok(()),
            },
        );
        let token = io.begin_open();
        assert!(io.seed_if_current(token, 0, initial.clone()));
        let controller = Rc::new(SettingsValueController {
            window: window.as_weak(),
            cache: cache.clone(),
            io: io.clone(),
            preview: RefCell::new(Some(initial)),
            devices: Rc::new(RefCell::new(Vec::new())),
            summary: Rc::new(RefCell::new(None)),
            apply_appearance: Rc::new(|_| {}),
        });
        wire(&window, controller);

        window.invoke_settings_paste_delay_changed(200);
        drain_until(&io, &step, 1);
        assert_eq!(window.get_settings_paste_delay_ms(), 150);
        assert_eq!(AppSettings::load(&db).unwrap().paste_delay_ms, 150);
        assert!(window.get_settings_save_error().contains("write rejected"));

        window.invoke_settings_paste_delay_changed(200);
        drain_until(&io, &step, 2);
        assert_eq!(window.get_settings_paste_delay_ms(), 200);
        assert_eq!(AppSettings::load(&db).unwrap().paste_delay_ms, 200);
        assert!(
            window
                .get_settings_save_error()
                .contains("native effect failed after commit")
        );
        {
            let cached = cache.borrow();
            let cached = cached
                .as_ref()
                .expect("cache after committed effect failure");
            assert_eq!(
                cached.dictation_polish_templates[0].label,
                "Committed polish template"
            );
            assert_eq!(
                cached.summary_templates[0].name,
                "Committed summary template"
            );
        }

        window.invoke_settings_paste_delay_changed(250);
        drain_until(&io, &step, 3);
        let stored_after_later_save = AppSettings::load(&db).unwrap();
        assert_eq!(stored_after_later_save.paste_delay_ms, 250);
        assert_eq!(
            stored_after_later_save.dictation_polish_templates[0].label,
            "Committed polish template"
        );
        assert_eq!(
            stored_after_later_save.summary_templates[0].name,
            "Committed summary template"
        );
        assert_eq!(window.get_settings_paste_delay_ms(), 200);
        assert!(!cache.is_known());
        assert!(
            window
                .get_settings_save_error()
                .contains("Stored settings are unknown: injected reread failure")
        );

        window.invoke_settings_feedback_sounds_volume_changed(23);
        drain_until(&io, &step, 4);
        assert_eq!(window.get_settings_paste_delay_ms(), 250);
        assert_eq!(window.get_settings_feedback_sounds_volume(), 23);
        let recovered = AppSettings::load(&db).unwrap();
        assert_eq!(recovered.paste_delay_ms, 250);
        assert_eq!(recovered.feedback_sounds_volume, 23);
        assert!(cache.is_known());
        assert!(window.get_settings_save_error().is_empty());

        // Legacy full-settings callbacks (calendar selection and microphone
        // list edits) use the same known-aware path. They must not revive an
        // older full snapshot after another unavailable reread.
        window.invoke_settings_paste_delay_changed(275);
        drain_until(&io, &step, 5);
        assert_eq!(AppSettings::load(&db).unwrap().paste_delay_ms, 275);
        assert_eq!(window.get_settings_paste_delay_ms(), 250);
        assert!(!cache.is_known());
        let load_db = db.clone();
        let save_db = db.clone();
        let outcome = save_field(
            &cache,
            move || AppSettings::load(&load_db),
            move |candidate| {
                let result = committed_result(candidate.save(&save_db));
                SettingsSaveOutcome::from_results(result, AppSettings::load(&save_db))
            },
            |settings| settings.calendar_selected_ids = vec!["calendar-a".into()],
        );
        assert!(matches!(
            outcome,
            SettingsSaveOutcome::Observed { result: Ok(()), .. }
        ));
        let stored = AppSettings::load(&db).unwrap();
        assert_eq!(stored.paste_delay_ms, 275);
        assert_eq!(stored.calendar_selected_ids, ["calendar-a"]);
        assert!(cache.is_known());
        assert!(window.get_settings_save_error().is_empty());

        let rejected_save_db = db.clone();
        let rejected = save_field(
            &cache,
            || unreachable!("the observed cache must avoid a fallback read"),
            move |_| SettingsSaveOutcome::Observed {
                settings: Box::new(AppSettings::load(&rejected_save_db).unwrap()),
                result: Err(SettingsSaveError::Rejected {
                    message: "legacy write rejected".into(),
                }),
            },
            |_| {},
        );
        assert!(matches!(
            rejected,
            SettingsSaveOutcome::Observed { result: Err(_), .. }
        ));
        assert!(
            window
                .get_settings_save_error()
                .contains("legacy write rejected")
        );
        let clear_save_db = db.clone();
        save_field(
            &cache,
            || unreachable!("the observed cache must avoid a fallback read"),
            move |candidate| {
                let result = committed_result(candidate.save(&clear_save_db));
                SettingsSaveOutcome::from_results(result, AppSettings::load(&clear_save_db))
            },
            |_| {},
        );
        assert!(window.get_settings_save_error().is_empty());
    }
}
