use objc2_service_management::{SMAppService, SMAppServiceStatus};

#[cfg(target_os = "macos")]
pub fn is_enabled() -> bool {
    let service = unsafe { SMAppService::mainAppService() };
    unsafe { service.status() == SMAppServiceStatus::Enabled }
}

#[cfg(target_os = "macos")]
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let service = unsafe { SMAppService::mainAppService() };
    let status = unsafe { service.status() };
    
    if enabled && status != SMAppServiceStatus::Enabled {
        unsafe {
            service.registerAndReturnError().map_err(|e| format!("Failed to register login item: {:?}", e))?;
        }
    } else if !enabled && status == SMAppServiceStatus::Enabled {
        unsafe {
            service.unregisterAndReturnError().map_err(|e| format!("Failed to unregister login item: {:?}", e))?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn is_launched_at_login() -> bool {
    use objc2_app_kit::NSApplication;
    use objc2_foundation::MainThreadMarker;
    // LaunchServices makes the app active if launched by the user from Finder.
    // If launched as a login item via SMAppService, it is launched in the background.
    if let Some(mtm) = MainThreadMarker::new() {
        let app = unsafe { NSApplication::sharedApplication(mtm) };
        let is_active = unsafe { app.isActive() };
        !is_active
    } else {
        false
    }
}
