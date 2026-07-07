use crate::permissions::{self, PermState, PermissionKind, PermissionStatus};

/// Cheap, non-prompting snapshot for the onboarding's initial render.
#[tauri::command]
#[specta::specta]
pub fn get_permission_status() -> Result<PermissionStatus, String> {
    Ok(permissions::snapshot())
}

/// Trigger the native prompt (or open System Settings) for one permission.
/// The probe opens a device, so it runs off the command thread.
#[tauri::command]
#[specta::specta]
pub async fn request_permission(kind: PermissionKind) -> Result<PermState, String> {
    tauri::async_runtime::spawn_blocking(move || permissions::request(kind))
        .await
        .map_err(|e| format!("Permission probe failed: {e}"))
}
