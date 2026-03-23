use tauri::{AppHandle, Emitter, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tracing::info;

use crate::state::AppState;

/// Get all settings as a JSON object
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let pairs = state.db.get_all_settings()?;
    let mut map = serde_json::Map::new();
    for (key, value_str) in pairs {
        let value: serde_json::Value =
            serde_json::from_str(&value_str).unwrap_or(serde_json::Value::String(value_str));
        map.insert(key, value);
    }
    Ok(serde_json::Value::Object(map))
}

/// Save a single setting (key + JSON-encoded value)
#[tauri::command]
pub fn save_setting(
    state: State<'_, AppState>,
    key: String,
    value: serde_json::Value,
) -> Result<(), String> {
    if key == "debug_transcription" {
        if let Some(enabled) = value.as_bool() {
            crate::debug::set_transcription_debug(enabled);
        }
    }
    let value_str = serde_json::to_string(&value).map_err(|e| format!("Serialize: {e}"))?;
    state.db.set_setting(&key, &value_str)
}

/// Register global shortcuts for toggle and push-to-talk dictation.
pub fn register_shortcuts(
    app: &AppHandle,
    toggle_shortcut: &str,
    ptt_shortcut: &str,
) -> Result<(), String> {
    let gs = app.global_shortcut();

    gs.unregister_all()
        .map_err(|e| format!("Unregister: {e}"))?;

    if !toggle_shortcut.is_empty() {
        gs.on_shortcut(toggle_shortcut, move |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                let _ = app.emit("shortcut-toggle", ());
            }
        })
        .map_err(|e| format!("Register toggle shortcut '{toggle_shortcut}': {e}"))?;
        info!(shortcut = toggle_shortcut, "Toggle shortcut registered");
    }

    if !ptt_shortcut.is_empty() {
        gs.on_shortcut(ptt_shortcut, move |app, _shortcut, event| {
            match event.state {
                ShortcutState::Pressed => {
                    let _ = app.emit("shortcut-ptt-start", ());
                }
                ShortcutState::Released => {
                    let _ = app.emit("shortcut-ptt-stop", ());
                }
            }
        })
        .map_err(|e| format!("Register PTT shortcut '{ptt_shortcut}': {e}"))?;
        info!(shortcut = ptt_shortcut, "Push-to-talk shortcut registered");
    }

    Ok(())
}

/// Update shortcut bindings at runtime.
#[tauri::command]
pub fn update_shortcuts(
    app: AppHandle,
    state: State<'_, AppState>,
    toggle_shortcut: String,
    ptt_shortcut: String,
) -> Result<(), String> {
    let toggle_json =
        serde_json::to_string(&toggle_shortcut).map_err(|e| format!("Serialize: {e}"))?;
    let ptt_json = serde_json::to_string(&ptt_shortcut).map_err(|e| format!("Serialize: {e}"))?;

    state.db.set_setting("shortcut_toggle", &toggle_json)?;
    state.db.set_setting("shortcut_push_to_talk", &ptt_json)?;

    register_shortcuts(&app, &toggle_shortcut, &ptt_shortcut)
}

/// Get current shortcut settings
#[tauri::command]
pub fn get_shortcuts(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let toggle = state
        .db
        .get_setting("shortcut_toggle")?
        .and_then(|v| serde_json::from_str::<String>(&v).ok())
        .unwrap_or_else(|| crate::DEFAULT_TOGGLE_SHORTCUT.to_string());

    let ptt = state
        .db
        .get_setting("shortcut_push_to_talk")?
        .and_then(|v| serde_json::from_str::<String>(&v).ok())
        .unwrap_or_default();

    Ok(serde_json::json!({
        "toggle": toggle,
        "push_to_talk": ptt,
    }))
}
