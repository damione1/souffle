use tauri::State;

use crate::settings::AppSettings;
use crate::state::AppState;
use crate::summary::{DictationPolishResult, check_providers, polish_dictation_text};

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
    state.db.add_dictation_entry(&id, &text, &timestamp)
}

/// Delete a single dictation entry
#[tauri::command]
#[specta::specta]
pub fn delete_dictation_entry(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.db.delete_dictation_entry(&id)
}

/// Clear all dictation history
#[tauri::command]
#[specta::specta]
pub fn clear_dictation_history(state: State<'_, AppState>) -> Result<(), String> {
    state.db.clear_dictation_entries()
}

/// Optional LLM polish pass for dictation text before paste/history.
#[tauri::command]
#[specta::specta]
pub async fn polish_dictation(
    state: State<'_, AppState>,
    text: String,
) -> Result<DictationPolishResult, String> {
    let settings = AppSettings::load(&state.db)?;
    let providers = check_providers(&settings.ollama_url).await;
    Ok(polish_dictation_text(
        &settings,
        &text,
        &providers.models,
    )
    .await)
}
