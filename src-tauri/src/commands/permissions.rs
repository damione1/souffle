use std::sync::Arc;

use tauri::State;

use crate::permissions::{
    self, PermState, PermissionKind, PermissionStatus, RepairAccessibilityResult,
};
use crate::state::AppState;

/// Cheap, non-prompting snapshot for the onboarding's initial render.
#[tauri::command]
#[specta::specta]
pub fn get_permission_status(state: State<'_, AppState>) -> Result<PermissionStatus, String> {
    Ok(permissions::snapshot(&state.db))
}

/// Trigger the native prompt (or open System Settings) for one permission.
/// The probe opens a device, so it runs off the command thread.
#[tauri::command]
#[specta::specta]
pub async fn request_permission(
    state: State<'_, AppState>,
    kind: PermissionKind,
) -> Result<PermState, String> {
    let db = Arc::clone(&state.db);
    tauri::async_runtime::spawn_blocking(move || {
        let result = permissions::request(kind);
        if kind == PermissionKind::Microphone {
            permissions::remember_microphone(&db, result);
        }
        result
    })
    .await
    .map_err(|e| format!("Permission probe failed: {e}"))
}

/// Clear a stale Accessibility TCC entry and re-prompt. Updating the app by
/// overwriting the .app bundle in place can leave System Settings showing
/// Souffle as granted while `AXIsProcessTrusted` still returns false, because
/// the TCC entry is keyed to the previous code-signing identity. Runs off
/// the command thread since it shells out and may block on the prompt.
#[tauri::command]
#[specta::specta]
pub async fn repair_accessibility_permission() -> Result<RepairAccessibilityResult, String> {
    tauri::async_runtime::spawn_blocking(permissions::repair_accessibility)
        .await
        .map_err(|e| format!("Accessibility repair failed: {e}"))?
}
