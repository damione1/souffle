use crate::permissions::{
    self, PermState, PermissionKind, PermissionStatus, RepairAccessibilityResult,
};

/// Cheap, non-prompting snapshot for the onboarding's initial render, and
/// the source of the panel's 600 ms poll.
///
/// Off the command thread even though every read is a status API: three of
/// them (`AXIsProcessTrusted`, `TCCAccessPreflight`, EventKit) are XPC round
/// trips to `tccd`, and a synchronous command runs on the main thread, where
/// a stalled `tccd` would freeze the window. Nothing here needs the main
/// thread — only *requests* do (SOU-122).
#[tauri::command]
#[specta::specta]
pub async fn get_permission_status() -> Result<PermissionStatus, String> {
    tauri::async_runtime::spawn_blocking(permissions::snapshot)
        .await
        .map_err(|e| format!("Permission status read failed: {e}"))
}

/// Trigger the native prompt (or open System Settings) for one permission.
/// Blocks until the user answers the dialog, so it runs off the command
/// thread.
#[tauri::command]
#[specta::specta]
pub async fn request_permission(kind: PermissionKind) -> Result<PermState, String> {
    tauri::async_runtime::spawn_blocking(move || permissions::request(kind))
        .await
        .map_err(|e| format!("Permission request failed: {e}"))
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
