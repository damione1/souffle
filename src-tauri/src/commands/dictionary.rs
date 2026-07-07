use tauri::State;

use crate::filter::DictionaryEntry;
use crate::state::AppState;

#[tauri::command]
#[specta::specta]
pub fn list_dictionary(state: State<'_, AppState>) -> Result<Vec<DictionaryEntry>, String> {
    state.db.list_dictionary_entries()
}

#[tauri::command]
#[specta::specta]
pub fn add_dictionary_entry(
    state: State<'_, AppState>,
    term: String,
    pronunciation: Option<String>,
    category: Option<String>,
) -> Result<DictionaryEntry, String> {
    let term = term.trim();
    if term.is_empty() {
        return Err("Term cannot be empty".into());
    }
    state
        .db
        .add_dictionary_entry(term, pronunciation.as_deref(), category.as_deref())
}

#[tauri::command]
#[specta::specta]
pub fn update_dictionary_entry(
    state: State<'_, AppState>,
    id: i64,
    term: String,
    pronunciation: Option<String>,
    category: Option<String>,
) -> Result<(), String> {
    let term = term.trim();
    if term.is_empty() {
        return Err("Term cannot be empty".into());
    }
    state
        .db
        .update_dictionary_entry(id, term, pronunciation.as_deref(), category.as_deref())
}

#[tauri::command]
#[specta::specta]
pub fn delete_dictionary_entry(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state.db.delete_dictionary_entry(id)
}

#[tauri::command]
#[specta::specta]
pub fn clear_dictionary(state: State<'_, AppState>) -> Result<(), String> {
    state.db.clear_dictionary()
}
