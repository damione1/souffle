use tauri::State;

use crate::settings::AppSettings;
use crate::state::AppState;
use crate::summary::{
    DictationPolishResult, check_providers, early_polish_dictation_result, polish_dictation_text,
};

/// List dictation history entries
#[tauri::command]
#[specta::specta]
pub fn list_dictation_entries(
    state: State<'_, AppState>,
    limit: Option<i64>,
) -> Result<Vec<crate::db::dictation::DictationEntry>, String> {
    state.db.list_dictation_entries(limit.unwrap_or(50))
}

/// Add a dictation history entry
#[tauri::command]
#[specta::specta]
pub fn add_dictation_entry(state: State<'_, AppState>, text: String) -> Result<(), String> {
    let id = uuid::Uuid::new_v4().to_string();
    let timestamp = chrono::Utc::now().to_rfc3339();
    state.db.add_dictation_entry(&id, &text, &timestamp)?;
    // The Idle transition also syncs the tray, but it fires before this write
    // (the frontend saves history only after stop resolves).
    sync_tray(&state);
    Ok(())
}

/// Delete a single dictation entry
#[tauri::command]
#[specta::specta]
pub fn delete_dictation_entry(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.db.delete_dictation_entry(&id)?;
    sync_tray(&state);
    Ok(())
}

/// Clear all dictation history
#[tauri::command]
#[specta::specta]
pub fn clear_dictation_history(state: State<'_, AppState>) -> Result<(), String> {
    state.db.clear_dictation_entries()?;
    sync_tray(&state);
    Ok(())
}

/// Optional LLM polish pass for dictation text before paste/history.
#[tauri::command]
#[specta::specta]
pub async fn polish_dictation(
    state: State<'_, AppState>,
    text: String,
    focused_app: Option<String>,
    rewrite_of: Option<String>,
) -> Result<DictationPolishResult, String> {
    let settings = AppSettings::load(&state.db)?;
    if let Some(result) = early_polish_dictation_result(&settings, &text) {
        return Ok(result);
    }

    let providers = check_providers(&settings.ollama_url).await;
    let dictionary = state.db.list_dictionary_entries()?;
    Ok(polish_dictation_text(
        &settings,
        &text,
        &providers.models,
        &dictionary,
        focused_app.as_deref(),
        rewrite_of.as_deref(),
    )
    .await)
}

/// Best-effort tray refresh so "Copy Last Transcription" tracks history.
/// No-op without a handle (tests, early startup) — same tolerance as
/// `apply_transition`.
fn sync_tray(state: &AppState) {
    if let (Ok(app), Ok(machine)) = (state.app_handle(), state.current_machine_state()) {
        crate::tray::sync(&app, &machine);
    }
}
