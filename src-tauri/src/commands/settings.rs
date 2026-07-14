use tauri::{AppHandle, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_specta::Event;
use tracing::info;

use crate::app_events::{ShortcutPttStart, ShortcutPttStop, ShortcutToggle};
use crate::settings::{AppSettings, ShortcutSettings};
use crate::state::AppState;

/// Get the typed application settings.
#[tauri::command]
#[specta::specta]
pub fn get_settings(state: State<'_, AppState>) -> Result<AppSettings, String> {
    AppSettings::load(&state.db)
}

/// Save the typed application settings.
#[tauri::command]
#[specta::specta]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AppSettings,
) -> Result<(), String> {
    let settings = settings.sanitize_for_save()?;
    settings.save(&state.db)?;
    crate::debug::set_transcription_debug(settings.debug_transcription);
    crate::logging::set_level(settings.log_level)?;
    state
        .engine_actor
        .set_unload_timeout(settings.model_unload_timeout_minutes);
    let _ = state.audio_cmd_sender.send(crate::state::AudioCommand::SetClamshellDevice(
        settings.clamshell_audio_device.clone(),
    ));
    let _ = state.audio_cmd_sender.send(crate::state::AudioCommand::SetInputPolicy {
        priority: settings.input_priority.clone(),
        allow_bluetooth_mic: settings.allow_bluetooth_mic,
    });
    // A locale change must relabel the tray menu immediately.
    if let Ok(machine) = state.current_machine_state() {
        crate::tray::sync(&app, &machine);
    }
    Ok(())
}

/// Register global shortcuts for toggle and push-to-talk dictation.
pub fn register_shortcuts(app: &AppHandle, shortcuts: &ShortcutSettings) -> Result<(), String> {
    let gs = app.global_shortcut();

    gs.unregister_all()
        .map_err(|e| format!("Unregister: {e}"))?;

    if !shortcuts.toggle.is_empty() {
        gs.on_shortcut(shortcuts.toggle.as_str(), move |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                let _ = ShortcutToggle.emit(app);
            }
        })
        .map_err(|e| format!("Register toggle shortcut '{}': {e}", shortcuts.toggle))?;
        info!(shortcut = shortcuts.toggle, "Toggle shortcut registered");
    }

    if !shortcuts.push_to_talk.is_empty() {
        gs.on_shortcut(
            shortcuts.push_to_talk.as_str(),
            move |app, _shortcut, event| match event.state {
                ShortcutState::Pressed => {
                    let _ = ShortcutPttStart.emit(app);
                }
                ShortcutState::Released => {
                    let _ = ShortcutPttStop.emit(app);
                }
            },
        )
        .map_err(|e| format!("Register PTT shortcut '{}': {e}", shortcuts.push_to_talk))?;
        info!(
            shortcut = shortcuts.push_to_talk,
            "Push-to-talk shortcut registered"
        );
    }

    Ok(())
}

/// Update shortcut bindings at runtime.
#[tauri::command]
#[specta::specta]
pub fn save_shortcuts(
    app: AppHandle,
    state: State<'_, AppState>,
    shortcuts: ShortcutSettings,
) -> Result<(), String> {
    let previous = ShortcutSettings::load(&state.db)?;
    let shortcuts = shortcuts.normalize()?;

    register_shortcuts(&app, &shortcuts)?;
    if let Err(e) = shortcuts.save(&state.db) {
        let _ = register_shortcuts(&app, &previous);
        return Err(e);
    }

    Ok(())
}

/// Get current shortcut settings
#[tauri::command]
#[specta::specta]
pub fn get_shortcuts(state: State<'_, AppState>) -> Result<ShortcutSettings, String> {
    ShortcutSettings::load(&state.db)
}
