use tauri::State;

use crate::db::snippets::SnippetEntry;
use crate::state::AppState;

#[tauri::command]
#[specta::specta]
pub fn list_snippets(state: State<'_, AppState>) -> Result<Vec<SnippetEntry>, String> {
    state.db.list_snippets()
}

#[tauri::command]
#[specta::specta]
pub fn add_snippet(
    state: State<'_, AppState>,
    trigger: &str,
    expansion: &str,
) -> Result<SnippetEntry, String> {
    state.db.add_snippet(trigger, expansion)
}

#[tauri::command]
#[specta::specta]
pub fn update_snippet(
    state: State<'_, AppState>,
    id: i64,
    trigger: &str,
    expansion: &str,
) -> Result<(), String> {
    state.db.update_snippet(id, trigger, expansion)
}

#[tauri::command]
#[specta::specta]
pub fn delete_snippet(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state.db.delete_snippet(id)
}
