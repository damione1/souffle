//! Settings values are controlled by Rust. This module wires the value callbacks
//! and owns the save/read/publication round trip, without querying catalogues.

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::rc::Rc;

use slint::ComponentHandle;
use souffle_lib::audio::AudioInputDevice;
use souffle_lib::commands::SettingsSaveOutcome;
use souffle_lib::settings::{AppSettings, Theme as SettingsTheme};
use souffle_lib::summary::SummaryProvidersStatus;

use crate::{MainWindow, Theme, audio_ui, ia_ui, model_ui, settings_ui};

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
    fn with_observed(window: &MainWindow, settings: AppSettings) -> Self {
        Self {
            snapshot: Rc::new(RefCell::new(Some(settings))),
            known: Rc::new(Cell::new(true)),
            window: window.as_weak(),
        }
    }

    pub(crate) fn borrow(&self) -> Ref<'_, Option<AppSettings>> {
        self.snapshot.borrow()
    }

    pub(crate) fn borrow_mut(&self) -> RefMut<'_, Option<AppSettings>> {
        self.snapshot.borrow_mut()
    }

    pub(crate) fn replace_observed(&self, settings: AppSettings) {
        *self.snapshot.borrow_mut() = Some(settings);
        self.known.set(true);
    }

    fn mark_unknown(&self) {
        self.known.set(false);
    }

    fn publish_save_status(&self, outcome: &SettingsSaveOutcome) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let message = match outcome {
            SettingsSaveOutcome::Observed { result, .. } => {
                result.as_ref().err().cloned().unwrap_or_default()
            }
            SettingsSaveOutcome::Unavailable { result, read_error } => match result {
                Ok(()) => format!("Stored settings are unknown: {read_error}"),
                Err(error) => format!("{error}. Stored settings are unknown: {read_error}"),
            },
        };
        window.set_settings_save_error(message.into());
    }

    #[cfg(test)]
    fn is_known(&self) -> bool {
        self.known.get()
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
pub(crate) fn save_field(
    cache: &SettingsCache,
    load: impl FnOnce() -> Result<AppSettings, String>,
    save: impl FnOnce(AppSettings) -> SettingsSaveOutcome,
    mutate: impl FnOnce(&mut AppSettings),
) -> SettingsSaveOutcome {
    let cached = cache.borrow().clone();
    let original = match (cache.known.get(), cached) {
        (true, Some(settings)) => settings,
        (_, cached) => match load() {
            Ok(mut settings) => {
                // These are the only intentionally optimistic cache values.
                // Recover them without reviving stale persisted fields.
                if let Some(cached) = cached {
                    settings.dictation_polish_templates = cached.dictation_polish_templates;
                    settings.summary_templates = cached.summary_templates;
                }
                settings
            }
            Err(read_error) => {
                cache.mark_unknown();
                let outcome = SettingsSaveOutcome::Unavailable {
                    result: Err("Settings could not be loaded before saving".into()),
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
    match &outcome {
        SettingsSaveOutcome::Observed { settings, result } => {
            let mut cached = settings.as_ref().clone();
            if result.is_err() {
                // These two fields still host the existing debounced drafts.
                // Keep them recoverable until SOU-201 separates their lifecycle.
                cached.dictation_polish_templates = original.dictation_polish_templates;
                cached.summary_templates = original.summary_templates;
            }
            cache.replace_observed(cached);
        }
        SettingsSaveOutcome::Unavailable { .. } => cache.mark_unknown(),
    }
    cache.publish_save_status(&outcome);
    outcome
}

pub(crate) struct SettingsValueController {
    window: slint::Weak<MainWindow>,
    cache: SettingsCache,
    devices: DeviceCache,
    summary: SummaryCache,
    load: Rc<dyn Fn() -> Result<AppSettings, String>>,
    save: Rc<dyn Fn(AppSettings) -> SettingsSaveOutcome>,
    apply_appearance: Rc<dyn Fn(bool)>,
}

impl SettingsValueController {
    pub(crate) fn new(
        window: &MainWindow,
        handle: crate::AppHandle,
        cache: SettingsCache,
        devices: DeviceCache,
        summary: SummaryCache,
    ) -> Rc<Self> {
        let load_handle = handle.clone();
        Rc::new(Self {
            window: window.as_weak(),
            cache,
            devices,
            summary,
            load: Rc::new(move || souffle_lib::commands::get_settings(load_handle.clone())),
            save: Rc::new(move |settings| {
                souffle_lib::commands::save_settings_observed(handle.clone(), settings)
            }),
            apply_appearance: Rc::new(souffle_lib::native::appearance::apply_resolved),
        })
    }

    fn apply(&self, mutate: impl FnOnce(&mut AppSettings)) {
        let previous_theme = if self.cache.known.get() {
            self.cache.borrow().as_ref().map(|settings| settings.theme)
        } else {
            None
        };
        let outcome = save_field(&self.cache, || (self.load)(), |s| (self.save)(s), mutate);
        let Some(window) = self.window.upgrade() else {
            return;
        };
        match outcome {
            SettingsSaveOutcome::Observed { settings, .. } => {
                let applied_dark = window.global::<Theme>().get_dark();
                let dark =
                    canonical_dark_after_save(settings.theme, previous_theme, applied_dark, || {
                        settings_ui::resolve_dark(SettingsTheme::System)
                    });
                self.project(&window, &settings);
                // Only the theme path touches AppKit, on the callback's UI thread.
                // Ordinary value publication performs no native or database I/O.
                if applied_dark != dark {
                    window.global::<Theme>().set_dark(dark);
                    (self.apply_appearance)(dark);
                }
            }
            SettingsSaveOutcome::Unavailable { .. } => {}
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
    let c = controller.clone();
    window.on_settings_theme_changed(move |value| {
        c.apply(|s| s.theme = settings_ui::theme_from_slint(value));
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
        c.apply(|s| s.theme = theme);
    });
    let c = controller.clone();
    window.on_settings_locale_changed(move |value| {
        c.apply(|s| s.locale = settings_ui::locale_from_slint(value).into());
    });
    let c = controller.clone();
    window.on_settings_paste_method_changed(move |value| {
        c.apply(|s| s.paste_method = settings_ui::paste_method_from_slint(value));
    });
    let c = controller.clone();
    window.on_settings_paste_delay_changed(move |value| {
        c.apply(|s| s.paste_delay_ms = value.max(0) as u64);
    });
    let c = controller.clone();
    window.on_settings_feedback_sounds_volume_changed(move |value| {
        c.apply(|s| s.feedback_sounds_volume = value.clamp(0, 100) as u32);
    });
    let c = controller.clone();
    window.on_settings_calendar_reminder_minutes_changed(move |value| {
        c.apply(|s| s.calendar_reminder_minutes = value.clamp(1, 30) as u32);
    });
    let c = controller.clone();
    window.on_settings_clamshell_device_changed(move |label| {
        let uid = audio_ui::resolve_device_uid(&c.devices.borrow(), &label);
        c.apply(|s| s.clamshell_audio_device = uid);
    });
    let c = controller.clone();
    window.on_settings_meeting_transcription_language_changed(move |value| {
        c.apply(|s| {
            s.meeting_transcription_language = settings_ui::meeting_language_from_slint(value)
        });
    });
    let c = controller.clone();
    window.on_settings_meeting_autostop_changed(move |label| {
        if let Some(minutes) = audio_ui::parse_minute_label(&label) {
            c.apply(|s| s.meeting_autostop_minutes = minutes);
        }
    });
    let c = controller.clone();
    window.on_settings_meeting_max_duration_changed(move |label| {
        if let Some(minutes) = audio_ui::parse_minute_label(&label) {
            c.apply(|s| s.meeting_max_duration_minutes = minutes);
        }
    });
    let c = controller.clone();
    window.on_settings_meeting_audio_retention_changed(move |value| {
        c.apply(|s| s.meeting_audio_retention = settings_ui::audio_retention_from_slint(value));
    });
    let c = controller.clone();
    window.on_settings_ollama_model_changed(move |label| {
        let id = c
            .summary
            .borrow()
            .as_ref()
            .and_then(|status| ia_ui::resolve_summary_model_id(&status.models, &label));
        if let Some(id) = id {
            c.apply(|s| s.ollama_model = id);
        }
    });
    let c = controller;
    window.on_settings_unload_timeout_changed(move |label| {
        if let Some(minutes) = model_ui::parse_unload_timeout_label(&label) {
            c.apply(|s| s.model_unload_timeout_minutes = minutes);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::Once;

    use slint::platform::{Platform, WindowAdapter, software_renderer::MinimalSoftwareWindow};
    use souffle_lib::audio::TransportType;
    use souffle_lib::db::Database;
    use souffle_lib::settings::{MeetingAudioRetention, MeetingTranscriptionLanguage, PasteMethod};
    use souffle_lib::summary::{SummaryModelDescriptor, SummaryProviderKind};

    struct TestPlatform;
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(Default::default()))
        }
    }

    fn test_window() -> MainWindow {
        static PLATFORM: Once = Once::new();
        PLATFORM.call_once(|| {
            slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        });
        MainWindow::new().unwrap()
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
            apple_intelligence_unavailable_reason: Some("test fixture".into()),
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
        cache: SettingsCache,
        devices: DeviceCache,
        summary: SummaryCache,
        fallback_loads: Rc<Cell<usize>>,
        saves: Rc<Cell<usize>>,
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

            let cache = SettingsCache::with_observed(&window, initial);
            let fallback_loads = Rc::new(Cell::new(0));
            let saves = Rc::new(Cell::new(0));
            let load_db = db.clone();
            let load_count = fallback_loads.clone();
            let save_db = db.clone();
            let save_count = saves.clone();
            let appearance = Rc::new(Cell::new(None));
            let projected_appearance = appearance.clone();
            let controller = Rc::new(SettingsValueController {
                window: window.as_weak(),
                cache: cache.clone(),
                devices: devices.clone(),
                summary: summary.clone(),
                load: Rc::new(move || {
                    load_count.set(load_count.get() + 1);
                    AppSettings::load(&load_db)
                }),
                save: Rc::new(move |settings| {
                    save_count.set(save_count.get() + 1);
                    let result = settings.save(&save_db);
                    SettingsSaveOutcome::from_results(result, AppSettings::load(&save_db))
                }),
                apply_appearance: Rc::new(move |dark| projected_appearance.set(Some(dark))),
            });
            wire(&window, controller);

            Self {
                window,
                db,
                cache,
                devices,
                summary,
                fallback_loads,
                saves,
                appearance,
                _directory: directory,
            }
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
        window.invoke_settings_ollama_model_changed("Model B".into());
        window.invoke_settings_unload_timeout_changed("5 min".into());

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

        assert_eq!(harness.saves.get(), 14);
        assert_eq!(harness.fallback_loads.get(), 0);
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
        assert_eq!(window.get_settings_unload_timeout_label().as_str(), "1 h");
        assert_eq!(
            AppSettings::load(&harness.db)
                .unwrap()
                .model_unload_timeout_minutes,
            60
        );
        assert_eq!(harness.saves.get(), 16);

        assert_failure_projection_contract();
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

        let cache = SettingsCache::with_observed(&window, initial);
        let step = Rc::new(Cell::new(0));
        let load_db = db.clone();
        let save_db = db.clone();
        let save_step = step.clone();
        let controller = Rc::new(SettingsValueController {
            window: window.as_weak(),
            cache: cache.clone(),
            devices: Rc::new(RefCell::new(Vec::new())),
            summary: Rc::new(RefCell::new(None)),
            load: Rc::new(move || AppSettings::load(&load_db)),
            save: Rc::new(move |candidate| {
                let current_step = save_step.get();
                save_step.set(current_step + 1);
                match current_step {
                    0 => SettingsSaveOutcome::Observed {
                        settings: Box::new(AppSettings::load(&save_db).unwrap()),
                        result: Err("write rejected".into()),
                    },
                    1 => {
                        candidate.save(&save_db).unwrap();
                        SettingsSaveOutcome::Observed {
                            settings: Box::new(AppSettings::load(&save_db).unwrap()),
                            result: Err("native effect failed after commit".into()),
                        }
                    }
                    2 => {
                        candidate.save(&save_db).unwrap();
                        SettingsSaveOutcome::Unavailable {
                            result: Ok(()),
                            read_error: "injected reread failure".into(),
                        }
                    }
                    3 => {
                        let result = candidate.save(&save_db);
                        SettingsSaveOutcome::from_results(result, AppSettings::load(&save_db))
                    }
                    4 => {
                        candidate.save(&save_db).unwrap();
                        SettingsSaveOutcome::Unavailable {
                            result: Ok(()),
                            read_error: "second injected reread failure".into(),
                        }
                    }
                    _ => unreachable!(),
                }
            }),
            apply_appearance: Rc::new(|_| {}),
        });
        wire(&window, controller);

        window.invoke_settings_paste_delay_changed(200);
        assert_eq!(window.get_settings_paste_delay_ms(), 150);
        assert_eq!(AppSettings::load(&db).unwrap().paste_delay_ms, 150);
        assert!(window.get_settings_save_error().contains("write rejected"));

        window.invoke_settings_paste_delay_changed(200);
        assert_eq!(window.get_settings_paste_delay_ms(), 200);
        assert_eq!(AppSettings::load(&db).unwrap().paste_delay_ms, 200);
        assert!(
            window
                .get_settings_save_error()
                .contains("native effect failed after commit")
        );

        window.invoke_settings_paste_delay_changed(250);
        assert_eq!(AppSettings::load(&db).unwrap().paste_delay_ms, 250);
        assert_eq!(window.get_settings_paste_delay_ms(), 200);
        assert!(!cache.is_known());
        assert!(
            window
                .get_settings_save_error()
                .contains("Stored settings are unknown: injected reread failure")
        );

        window.invoke_settings_feedback_sounds_volume_changed(23);
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
        assert_eq!(AppSettings::load(&db).unwrap().paste_delay_ms, 275);
        assert_eq!(window.get_settings_paste_delay_ms(), 250);
        assert!(!cache.is_known());
        let load_db = db.clone();
        let save_db = db.clone();
        let outcome = save_field(
            &cache,
            move || AppSettings::load(&load_db),
            move |candidate| {
                let result = candidate.save(&save_db);
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
                result: Err("legacy write rejected".into()),
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
                let result = candidate.save(&clear_save_db);
                SettingsSaveOutcome::from_results(result, AppSettings::load(&clear_save_db))
            },
            |_| {},
        );
        assert!(window.get_settings_save_error().is_empty());
    }
}
