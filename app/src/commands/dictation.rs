use std::sync::Arc;

use crate::settings::AppSettings;
use crate::state::AppState;
use crate::summary::{
    DictationPolishResult, check_providers, early_polish_dictation_result, polish_dictation_text,
};

/// List dictation history entries
pub fn list_dictation_entries(
    state: Arc<AppState>,
    limit: Option<i64>,
) -> Result<Vec<crate::db::dictation::DictationEntry>, String> {
    state.db.list_dictation_entries(limit.unwrap_or(50))
}

/// Add a dictation history entry. Returns the generated id so a later polish
/// pass can update the same row instead of inserting a second one.
pub fn add_dictation_entry(state: Arc<AppState>, text: String) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let timestamp = chrono::Utc::now().to_rfc3339();
    state.db.add_dictation_entry(&id, &text, &timestamp)?;
    // The Idle transition also syncs the tray, but it fires before this write
    // (the frontend saves history only after stop resolves).
    sync_tray(&state);
    Ok(id)
}

/// Replace the text of an existing dictation entry (e.g. after polish).
pub fn update_dictation_entry(
    state: Arc<AppState>,
    id: String,
    text: String,
) -> Result<(), String> {
    state.db.update_dictation_entry(&id, &text)?;
    sync_tray(&state);
    Ok(())
}

/// Delete a single dictation entry
pub fn delete_dictation_entry(state: Arc<AppState>, id: String) -> Result<(), String> {
    state.db.delete_dictation_entry(&id)?;
    sync_tray(&state);
    Ok(())
}

/// Clear all dictation history
pub fn clear_dictation_history(state: Arc<AppState>) -> Result<(), String> {
    state.db.clear_dictation_entries()?;
    sync_tray(&state);
    Ok(())
}

/// Optional LLM polish pass for dictation text before paste/history.
pub async fn polish_dictation(
    state: Arc<AppState>,
    text: String,
    focused_app: Option<String>,
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
    )
    .await)
}

/// Best-effort tray refresh so "Copy Last Transcription" tracks history.
fn sync_tray(state: &AppState) {
    if let Ok(machine) = state.current_machine_state() {
        crate::tray::sync(state, &machine);
    }
}
