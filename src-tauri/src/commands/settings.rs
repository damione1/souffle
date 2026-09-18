use std::sync::{Arc, Mutex};

use crate::db::Database;
use crate::lock_ext::MutexExt;
use crate::settings::{AppSettings, PreparedSettingsSaveError, ShortcutSettings};
use crate::state::AppState;
use crate::state_machine::AppStateMachine;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsEffectFailure {
    Autostart { message: String },
    Logging { message: String },
}

impl SettingsEffectFailure {
    fn user_message(&self) -> &str {
        match self {
            Self::Autostart { message } | Self::Logging { message } => message,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsSaveError {
    Rejected { message: String },
    NotCommitted { message: String },
    EffectFailedAfterCommit { cause: SettingsEffectFailure },
}

impl SettingsSaveError {
    pub fn user_message(&self) -> String {
        match self {
            Self::Rejected { message } => message.clone(),
            Self::NotCommitted { message } => message.clone(),
            Self::EffectFailedAfterCommit { cause } => {
                format!(
                    "Settings were saved, but a native effect failed: {}",
                    cause.user_message()
                )
            }
        }
    }

    pub fn committed(&self) -> bool {
        match self {
            Self::Rejected { .. } | Self::NotCommitted { .. } => false,
            Self::EffectFailedAfterCommit { .. } => true,
        }
    }
}

impl std::fmt::Display for SettingsSaveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.user_message())
    }
}

impl std::error::Error for SettingsSaveError {}

pub type SettingsSaveResult = Result<(), SettingsSaveError>;

/// The typed save result and the observed durable state are independent: a
/// native effect can fail after the atomic database commit, and the re-read can
/// fail independently of both.
#[derive(Debug)]
pub enum SettingsSaveOutcome {
    Observed {
        settings: Box<AppSettings>,
        result: SettingsSaveResult,
    },
    Unavailable {
        result: SettingsSaveResult,
        read_error: String,
    },
}

impl SettingsSaveOutcome {
    pub fn from_results(result: SettingsSaveResult, observed: Result<AppSettings, String>) -> Self {
        match observed {
            Ok(settings) => Self::Observed {
                settings: Box::new(settings),
                result,
            },
            Err(read_error) => Self::Unavailable { result, read_error },
        }
    }
}

/// Re-read even on failure: the atomic snapshot may have committed before a
/// native effect failed.
pub fn save_settings_observed(state: Arc<AppState>, settings: AppSettings) -> SettingsSaveOutcome {
    let result = save_settings(state.clone(), settings);
    SettingsSaveOutcome::from_results(result, get_settings(state))
}

/// Get the typed application settings.
pub fn get_settings(state: Arc<AppState>) -> Result<AppSettings, String> {
    let mut settings = AppSettings::load_read_only(&state.db)?;
    #[cfg(target_os = "macos")]
    {
        settings.autostart_enabled = crate::autostart::is_enabled();
    }
    Ok(settings)
}

/// Save the typed application settings.
pub fn save_settings(state: Arc<AppState>, settings: AppSettings) -> SettingsSaveResult {
    save_settings_with_effects(&state.db, &state.machine, settings, |settings| {
        apply_settings_effects(&state, settings)
    })
}

fn save_settings_with_effects(
    db: &Database,
    machine: &Mutex<AppStateMachine>,
    settings: AppSettings,
    apply_effects: impl FnOnce(&AppSettings) -> Result<(), SettingsEffectFailure>,
) -> SettingsSaveResult {
    let settings = settings
        .prepare_for_save()
        .map_err(|message| SettingsSaveError::Rejected { message })?;

    // Keep the canonical machine stable from the decision through the SQLite
    // commit. Native/window effects run only after this guard is dropped.
    {
        let machine = machine
            .acquire()
            .map_err(|message| SettingsSaveError::NotCommitted { message })?;
        settings
            .save_prepared_with_recording_guard(db, machine.is_recording())
            .map_err(|error| match error {
                PreparedSettingsSaveError::ModelChangeRejected => SettingsSaveError::Rejected {
                    message: "Cannot change the transcription model while recording".into(),
                },
                PreparedSettingsSaveError::Persistence { message } => {
                    SettingsSaveError::NotCommitted { message }
                }
            })?;
    }

    apply_effects(&settings).map_err(|cause| SettingsSaveError::EffectFailedAfterCommit { cause })
}

fn apply_settings_effects(
    state: &Arc<AppState>,
    settings: &AppSettings,
) -> Result<(), SettingsEffectFailure> {
    #[cfg(target_os = "macos")]
    {
        if settings.autostart_enabled != crate::autostart::is_enabled() {
            crate::autostart::set_enabled(settings.autostart_enabled)
                .map_err(|message| SettingsEffectFailure::Autostart { message })?;
        }
    }

    crate::debug::set_transcription_debug(settings.debug_transcription);
    crate::logging::set_level(settings.log_level)
        .map_err(|message| SettingsEffectFailure::Logging { message })?;
    state
        .engine_actor
        .set_unload_timeout(settings.model_unload_timeout_minutes);
    let _ = state
        .audio_cmd_sender
        .send(crate::state::AudioCommand::SetClamshellDevice(
            settings.clamshell_audio_device.clone(),
        ));
    let _ = state
        .audio_cmd_sender
        .send(crate::state::AudioCommand::SetInputPolicy {
            priority: settings.input_priority.clone(),
            allow_bluetooth_mic: settings.allow_bluetooth_mic,
        });
    crate::pill::set_hidden(settings.pill_hidden);
    // A locale change must relabel the tray menu immediately. Hide/show of
    // the recording overlay is applied on the same pass.
    if let Ok(machine) = state.current_machine_state() {
        crate::pill::sync(state, &machine);
        crate::tray::sync(state, &machine);
    }
    Ok(())
}

/// Register global shortcuts for toggle and push-to-talk dictation.
pub fn register_shortcuts(
    state: &Arc<AppState>,
    shortcuts: &ShortcutSettings,
) -> Result<(), String> {
    crate::native::shortcuts::register_shortcuts(state, shortcuts)
}

/// Update shortcut bindings at runtime.
pub fn save_shortcuts(state: Arc<AppState>, shortcuts: ShortcutSettings) -> Result<(), String> {
    let previous = ShortcutSettings::load(&state.db)?;
    let shortcuts = shortcuts.normalize()?;

    register_shortcuts(&state, &shortcuts)?;
    if let Err(e) = shortcuts.save(&state.db) {
        let _ = register_shortcuts(&state, &previous);
        return Err(e);
    }

    Ok(())
}

/// The shipped defaults, with nothing read from the database.
///
/// `get_settings` returns the *effective* settings, so it is no help when the
/// database does not exist yet. The UI needs a starting point before its
/// first successful read, and this is it: `AppSettings::default()` stays the
/// only declaration of those values.
pub fn get_default_settings() -> AppSettings {
    AppSettings::default()
}

/// Get current shortcut settings
pub fn get_shortcuts(state: Arc<AppState>) -> Result<ShortcutSettings, String> {
    ShortcutSettings::load(&state.db)
}

/// Accelerators that are registered through the `CGEventTap` rather than
/// a combo shortcut. The settings UI needs the list to warn that a binding
/// will require Accessibility.
pub fn get_native_shortcuts() -> Vec<String> {
    crate::modifier_shortcut::NATIVE_SHORTCUTS
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

/// The numeric choices the settings UI may offer. Declared in
/// `settings::SettingsOptions`, next to the bounds `sanitize_for_save`
/// validates them against, so the components do not restate them.
pub fn get_settings_options() -> crate::settings::SettingsOptions {
    crate::settings::SettingsOptions::current()
}

/// Last native PTT `CGEventTap` install status, for a UI that queries after
/// the transition already happened. `None` before the first install attempt
/// (including the startup delay).
pub fn get_modifier_tap_status() -> Option<crate::app_events::ModifierTapStatus> {
    crate::modifier_shortcut::modifier_tap_status()
}

/// Open the macOS System Settings to the Apple Intelligence & Siri pane.
pub fn open_apple_intelligence_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.Siri-Settings.extension")
        .spawn();
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::{Arc, Barrier, Mutex};

    use super::{SettingsEffectFailure, SettingsSaveError, save_settings_with_effects};
    use crate::audio::InputPriority;
    use crate::engine::default_transcription_profile;
    use crate::settings::{AppSettings, MeetingTranscriptionLanguage, Theme};
    use crate::state_machine::AppStateMachine;
    use crate::test_helpers::fixtures::test_db;

    fn recording_machine() -> AppStateMachine {
        AppStateMachine::RecordingDictation {
            profile: default_transcription_profile(),
            session_id: 7,
        }
    }

    #[test]
    fn recording_model_refusal_precedes_every_database_and_native_mutation() {
        let (db, _dir) = test_db();
        let initial = AppSettings::default();
        initial.save(&db).expect("save initial settings");
        let mut before = db.get_all_settings().expect("read initial settings");
        before.sort();

        let candidate = AppSettings {
            theme: Theme::Dark,
            locale: "fr".into(),
            transcription_model_id: "stt-2.6b-en".into(),
            ..initial
        };
        let effects_applied = Cell::new(false);
        let result = save_settings_with_effects(
            &db,
            &Mutex::new(recording_machine()),
            candidate.clone(),
            |_| {
                effects_applied.set(true);
                Ok(())
            },
        );

        assert!(matches!(result, Err(SettingsSaveError::Rejected { .. })));
        assert!(!effects_applied.get());
        let mut after = db.get_all_settings().expect("read rejected settings");
        after.sort();
        assert_eq!(after, before, "a machine refusal must change no key");

        let result =
            save_settings_with_effects(&db, &Mutex::new(AppStateMachine::Idle), candidate, |_| {
                Ok(())
            });
        assert_eq!(result, Ok(()));
        let stored = AppSettings::load(&db).expect("load idle settings");
        assert_eq!(stored.transcription_model_id, "stt-2.6b-en");
        assert_eq!(stored.theme, Theme::Dark);
        assert_eq!(stored.locale, "fr");
    }

    #[test]
    fn recording_allows_theme_locale_language_and_microphone_policy_changes() {
        let (db, _dir) = test_db();
        // A model selection is persisted before its asynchronous load starts.
        // Hot settings must remain writable if recording begins on the still
        // active previous profile during that transition window.
        let initial = AppSettings {
            transcription_model_id: "stt-2.6b-en".into(),
            ..AppSettings::default()
        };
        initial.save(&db).expect("save initial settings");
        let candidate = AppSettings {
            theme: Theme::Dark,
            locale: "fr".into(),
            meeting_transcription_language: MeetingTranscriptionLanguage::Fr,
            allow_bluetooth_mic: true,
            input_priority: InputPriority {
                priorities: vec!["preferred-mic".into()],
                ..InputPriority::default()
            },
            ..initial
        };

        let result = save_settings_with_effects(
            &db,
            &Mutex::new(recording_machine()),
            candidate,
            |_| Ok(()),
        );
        assert_eq!(result, Ok(()));
        let stored = AppSettings::load(&db).expect("load hot settings");
        assert_eq!(stored.theme, Theme::Dark);
        assert_eq!(stored.locale, "fr");
        assert_eq!(
            stored.meeting_transcription_language,
            MeetingTranscriptionLanguage::Fr
        );
        assert!(stored.allow_bluetooth_mic);
        assert_eq!(stored.input_priority.priorities, ["preferred-mic"]);
        assert_eq!(stored.transcription_model_id, "stt-2.6b-en");
    }

    #[test]
    fn native_effect_failure_reports_that_the_commit_succeeded() {
        let (db, _dir) = test_db();
        let initial = AppSettings::default();
        initial.save(&db).expect("save initial settings");
        let candidate = AppSettings {
            paste_delay_ms: 250,
            ..initial
        };

        let result =
            save_settings_with_effects(&db, &Mutex::new(AppStateMachine::Idle), candidate, |_| {
                Err(SettingsEffectFailure::Logging {
                    message: "injected native effect failure".into(),
                })
            });
        let error = result.expect_err("native effect must fail");
        assert_eq!(
            error,
            SettingsSaveError::EffectFailedAfterCommit {
                cause: SettingsEffectFailure::Logging {
                    message: "injected native effect failure".into()
                }
            }
        );
        assert!(error.committed());
        assert!(error.user_message().contains("Settings were saved"));
        assert_eq!(
            AppSettings::load(&db)
                .expect("load committed settings")
                .paste_delay_ms,
            250
        );
    }

    #[test]
    fn sqlite_persistence_failure_is_typed_and_skips_native_effects() {
        let (db, _dir) = test_db();
        let initial = AppSettings::default();
        initial.save(&db).expect("save initial settings");
        let mut before = db.get_all_settings().expect("read initial settings");
        before.sort();
        db.execute_settings_sql_for_test(
            r#"
            CREATE TRIGGER fail_calendar_integration_write
            BEFORE INSERT ON settings
            WHEN NEW.key = 'calendar_integration_enabled'
            BEGIN
                SELECT RAISE(ABORT, 'injected persistence failure');
            END;
            "#,
        )
        .expect("install failure trigger");

        let effects_applied = Cell::new(false);
        let candidate = AppSettings {
            theme: Theme::Dark,
            calendar_integration_enabled: true,
            ..initial
        };
        let result =
            save_settings_with_effects(&db, &Mutex::new(AppStateMachine::Idle), candidate, |_| {
                effects_applied.set(true);
                Ok(())
            });
        let error = result.expect_err("persistence must fail");
        assert!(matches!(error, SettingsSaveError::NotCommitted { .. }));
        assert!(!error.committed());
        assert!(!effects_applied.get());
        let mut after = db.get_all_settings().expect("read settings after failure");
        after.sort();
        assert_eq!(after, before);
    }

    #[test]
    fn queued_recording_transition_is_seen_before_the_model_commit() {
        let (db, _dir) = test_db();
        let db = Arc::new(db);
        let initial = AppSettings::default();
        initial.save(&db).expect("save initial settings");
        let mut before = db.get_all_settings().expect("read initial settings");
        before.sort();

        let machine = Arc::new(Mutex::new(AppStateMachine::Idle));
        let mut transition = machine.lock().expect("lock machine before save");
        let started = Arc::new(Barrier::new(2));
        let worker_db = Arc::clone(&db);
        let worker_machine = Arc::clone(&machine);
        let worker_started = Arc::clone(&started);
        let candidate = AppSettings {
            theme: Theme::Dark,
            transcription_model_id: "stt-2.6b-en".into(),
            ..initial
        };
        let worker = std::thread::spawn(move || {
            worker_started.wait();
            save_settings_with_effects(&worker_db, &worker_machine, candidate, |_| Ok(()))
        });

        started.wait();
        *transition = recording_machine();
        drop(transition);

        let result = worker.join().expect("join settings save");
        assert!(matches!(result, Err(SettingsSaveError::Rejected { .. })));
        let mut after = db.get_all_settings().expect("read settings after refusal");
        after.sort();
        assert_eq!(after, before, "the transition must win before commit");
    }

    #[test]
    fn poisoned_machine_lock_reports_not_committed_without_mutation() {
        let (db, _dir) = test_db();
        let initial = AppSettings::default();
        initial.save(&db).expect("save initial settings");
        let mut before = db.get_all_settings().expect("read initial settings");
        before.sort();

        let machine = Arc::new(Mutex::new(AppStateMachine::Idle));
        let poison = Arc::clone(&machine);
        let _ = std::thread::spawn(move || {
            let _guard = poison.lock().expect("lock machine for poisoning");
            panic!("inject poisoned machine lock");
        })
        .join();

        let effects_applied = Cell::new(false);
        let candidate = AppSettings {
            theme: Theme::Dark,
            ..initial
        };
        let result = save_settings_with_effects(&db, &machine, candidate, |_| {
            effects_applied.set(true);
            Ok(())
        });
        let error = result.expect_err("poisoned machine lock must fail");
        assert!(matches!(error, SettingsSaveError::NotCommitted { .. }));
        assert!(!error.committed());
        assert!(!effects_applied.get());
        let mut after = db
            .get_all_settings()
            .expect("read settings after lock failure");
        after.sort();
        assert_eq!(after, before);
    }
}
