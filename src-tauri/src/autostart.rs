//! Login item registration through `SMAppService` (macOS 13+, SOU-036).
//!
//! The service is the source of truth: the user can remove the entry from
//! System Settings > General > Login Items behind the app's back, so callers
//! read `is_enabled` from the system instead of trusting the stored setting.
//! `SMAppService` never triggers a TCC prompt, unlike the System Events
//! automation path `tauri-plugin-autostart` can take.

#[cfg(target_os = "macos")]
use objc2_service_management::{SMAppService, SMAppServiceStatus};

/// Whether the main app is currently registered as a login item, as reported
/// by the system.
#[cfg(target_os = "macos")]
pub fn is_enabled() -> bool {
    let service = unsafe { SMAppService::mainAppService() };
    unsafe { service.status() == SMAppServiceStatus::Enabled }
}

/// Register or unregister the main app as a login item. Only touches the
/// service when the requested state differs from the system's, so a re-save
/// with an unchanged toggle stays a no-op.
#[cfg(target_os = "macos")]
pub fn set_enabled(enabled: bool) -> Result<(), String> {
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
