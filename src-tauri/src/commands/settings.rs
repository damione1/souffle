use std::sync::Arc;

use crate::settings::{AppSettings, ShortcutSettings};
use crate::state::AppState;

/// The write result and the observed durable state are independent: a native
/// effect can fail after the database was written. Never infer rollback from Err.
#[derive(Debug)]
pub enum SettingsSaveOutcome {
    Observed {
        settings: Box<AppSettings>,
        result: Result<(), String>,
    },
    Unavailable {
        result: Result<(), String>,
        read_error: String,
    },
}

impl SettingsSaveOutcome {
    pub fn from_results(result: Result<(), String>, observed: Result<AppSettings, String>) -> Self {
        match observed {
            Ok(settings) => Self::Observed {
                settings: Box::new(settings),
                result,
            },
            Err(read_error) => Self::Unavailable { result, read_error },
        }
    }
}

/// Re-read even on failure: save_settings may already have committed some keys.
pub fn save_settings_observed(state: Arc<AppState>, settings: AppSettings) -> SettingsSaveOutcome {
    let result = save_settings(state.clone(), settings);
    SettingsSaveOutcome::from_results(result, get_settings(state))
}

/// Get the typed application settings.
pub fn get_settings(state: Arc<AppState>) -> Result<AppSettings, String> {
    let mut settings = AppSettings::load(&state.db)?;
    #[cfg(target_os = "macos")]
    {
        settings.autostart_enabled = crate::autostart::is_enabled();
    }
    Ok(settings)
}

/// Save the typed application settings.
pub fn save_settings(state: Arc<AppState>, settings: AppSettings) -> Result<(), String> {
    let mut settings = settings.sanitize_for_save()?;
    let stored = AppSettings::load(&state.db)?;

    #[cfg(target_os = "macos")]
    {
        if settings.autostart_enabled != crate::autostart::is_enabled() {
            crate::autostart::set_enabled(settings.autostart_enabled)?;
        }
    }

    let recording = state
        .current_machine_state()
        .map(|machine| machine.is_recording())
        .unwrap_or(false);
    let pinned = settings.pin_transcription_while_recording(&stored, recording);
    settings.save(&state.db)?;
    crate::debug::set_transcription_debug(settings.debug_transcription);
    crate::logging::set_level(settings.log_level)?;
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
        crate::pill::sync(&state, &machine);
        crate::tray::sync(&state, &machine);
    }
    if pinned {
        return Err("Cannot change the transcription model while recording".into());
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
