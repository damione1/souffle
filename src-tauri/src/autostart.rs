//! Login item registration through `SMAppService` (macOS 13+, SOU-036).
//!
//! The service is the source of truth: the user can remove the entry from
//! System Settings > General > Login Items behind the app's back, so callers
//! read `is_enabled` from the system instead of trusting the stored setting.
//! `SMAppService` never triggers a TCC prompt, unlike the System Events
//! automation path `tauri-plugin-autostart` can take.

#[cfg(target_os = "macos")]
use objc2_service_management::{SMAppService, SMAppServiceStatus};

#[cfg(target_os = "macos")]
static SERVICE_ACCESS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Whether the main app is currently registered as a login item, as reported
/// by the system.
#[cfg(target_os = "macos")]
pub fn is_enabled() -> bool {
    // SMAppService is a ServiceManagement/XPC object, not AppKit UI. Its SDK
    // declaration has no main-thread actor/annotation. Settings therefore
    // calls it from its serialized I/O worker so a slow status request cannot
    // block Slint; this lock prevents concurrent access from older call sites.
    let _guard = SERVICE_ACCESS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let service = unsafe { SMAppService::mainAppService() };
    unsafe { service.status() == SMAppServiceStatus::Enabled }
}

/// Register or unregister the main app as a login item. Only touches the
/// service when the requested state differs from the system's, so a re-save
/// with an unchanged toggle stays a no-op.
#[cfg(target_os = "macos")]
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let _guard = SERVICE_ACCESS
        .lock()
        .map_err(|_| "ServiceManagement access lock is poisoned".to_string())?;
    let service = unsafe { SMAppService::mainAppService() };
    let status = unsafe { service.status() };

    if enabled && status != SMAppServiceStatus::Enabled {
        unsafe {
            service
                .registerAndReturnError()
                .map_err(|e| format!("Failed to register login item: {e:?}"))?;
        }
    } else if !enabled && status == SMAppServiceStatus::Enabled {
        unsafe {
            service
                .unregisterAndReturnError()
                .map_err(|e| format!("Failed to unregister login item: {e:?}"))?;
        }
    }
    Ok(())
}
