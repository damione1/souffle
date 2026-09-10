use tauri::State;

use crate::db::snippets::SnippetEntry;
use crate::state::AppState;

/// Lists all voice snippets from the database.
#[tauri::command]
#[specta::specta]
pub fn list_snippets(state: State<'_, AppState>) -> Result<Vec<SnippetEntry>, String> {
    state.db.list_snippets()
}

/// Adds a new voice snippet (spoken trigger → pasted expansion).
#[tauri::command]
#[specta::specta]
pub fn add_snippet(
    state: State<'_, AppState>,
    trigger: &str,
    expansion: &str,
) -> Result<SnippetEntry, String> {
    let trigger = trigger.trim();
    if trigger.is_empty() {
        return Err("Trigger cannot be empty".into());
    }
    if expansion.trim().is_empty() {
        return Err("Expansion cannot be empty".into());
    }
    state.db.add_snippet(trigger, expansion)
}

/// Updates an existing voice snippet's trigger and expansion.
#[tauri::command]
#[specta::specta]
pub fn update_snippet(
    state: State<'_, AppState>,
    id: i64,
    trigger: &str,
    expansion: &str,
) -> Result<(), String> {
    let trigger = trigger.trim();
    if trigger.is_empty() {
        return Err("Trigger cannot be empty".into());
    }
    if expansion.trim().is_empty() {
        return Err("Expansion cannot be empty".into());
    }
    state.db.update_snippet(id, trigger, expansion)
}

/// Deletes a specific voice snippet by ID.
#[tauri::command]
#[specta::specta]
pub fn delete_snippet(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state.db.delete_snippet(id)
}
