use tauri::State;

use crate::diagnostics::{collect_bundle, format_bundle_text, tail_log_file, DiagnosticsBundle};
use crate::settings::AppSettings;
use crate::state::AppState;
use crate::update_check::UpdateCheckResult;

/// Return the last N lines from the active rolling log file.
#[tauri::command]
#[specta::specta]
pub fn get_log_tail(max_lines: u32) -> Result<String, String> {
    let lines = max_lines.clamp(1, 500) as usize;
    tail_log_file(lines)
}

/// Paths and runtime snapshot for the copy-diagnostics action.
#[tauri::command]
#[specta::specta]
pub fn get_diagnostics_bundle(state: State<'_, AppState>) -> Result<DiagnosticsBundle, String> {
    let settings = AppSettings::load(&state.db)?;
    Ok(collect_bundle(&state, &settings))
}

/// Full diagnostics text (bundle + recent log tail) for clipboard copy.
#[tauri::command]
#[specta::specta]
pub fn get_diagnostics_text(state: State<'_, AppState>) -> Result<String, String> {
    let settings = AppSettings::load(&state.db)?;
    let bundle = collect_bundle(&state, &settings);
    let log_tail = tail_log_file(200).unwrap_or_else(|e| format!("(log tail unavailable: {e})"));
    Ok(format_bundle_text(&bundle, &log_tail))
}

/// App version string from the running binary.
#[tauri::command]
#[specta::specta]
pub fn get_app_version() -> String {
    crate::update_check::current_version()
}

/// Check GitHub releases for a newer version. Network errors are returned in
/// the result payload so the UI can show a soft failure.
#[tauri::command]
#[specta::specta]
pub async fn check_for_updates() -> Result<UpdateCheckResult, String> {
    tauri::async_runtime::spawn_blocking(crate::update_check::check_for_updates)
        .await
        .map_err(|e| format!("Update check task failed: {e}"))
}

/// Release notes for a specific installed version tag (What's New). Returns
/// `None` when the tag is missing or the network request fails.
#[tauri::command]
#[specta::specta]
pub async fn get_release_notes_for_version(version: String) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::update_check::release_notes_for_version(&version)
    })
    .await
    .map_err(|e| format!("Release notes task failed: {e}"))
}
