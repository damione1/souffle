//! macOS power-state integration:
//! - Sleep/wake observers via `NSWorkspace` notifications, so an active
//!   recording is stopped cleanly before CoreAudio IO goes dead under system
//!   sleep, and the frontend can offer to resume once it wakes back up.
//! - Clamshell (lid closed with an external display attached) detection, so
//!   a configured "clamshell microphone" preference can be applied when the
//!   built-in mic goes away and macOS switches the default input.
//!
//! All AppKit interop lives here behind a narrow safe API; callers never
//! touch objc2 types directly.

use tracing::info;

/// Install `NSWorkspace` observers for system sleep/wake on the current
/// thread. Must be called once, from the Tauri setup closure (main thread) —
/// `NSWorkspace` notifications are posted on the main thread, and passing no
/// operation queue below runs the block synchronously there, matching that
/// requirement without an extra thread hop.
#[cfg(target_os = "macos")]
pub fn install_sleep_observers(
    on_will_sleep: impl Fn() + Send + 'static,
    on_did_wake: impl Fn() + Send + 'static,
) {
    use std::ptr::NonNull;

    use objc2_app_kit::{NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceWillSleepNotification};
    use objc2_foundation::NSNotification;

    let center = NSWorkspace::sharedWorkspace().notificationCenter();

    let will_sleep_block = block2::RcBlock::new(move |_note: NonNull<NSNotification>| {
        info!(lid_closed = is_clamshell(), "System will sleep");
        on_will_sleep();
    });
    let did_wake_block = block2::RcBlock::new(move |_note: NonNull<NSNotification>| {
        info!(lid_closed = is_clamshell(), "System woke up");
        on_did_wake();
    });

    // SAFETY: the blocks take exactly one `NSNotification*` argument and
    // return nothing, matching `addObserverForName:object:queue:usingBlock:`;
    // `name` is one of Apple's documented NSWorkspace notification constants
    // and `object`/`queue` are `nil`, both explicitly allowed by the API.
    unsafe {
        // The returned observer token must be kept alive for the app's
        // lifetime, or the observer is torn down as soon as it drops. There
        // is no natural long-lived owner for it here (this runs once during
        // Tauri setup and never returns to a caller that could hold it), so
        // both tokens are deliberately leaked instead of stashed in a static.
        // The notification center keeps its own retained copy of each block,
        // so the local `RcBlock`s are safe to drop normally once registered.
        let will_sleep_token = center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceWillSleepNotification),
            None,
            None,
            &will_sleep_block,
        );
        std::mem::forget(will_sleep_token);

        let did_wake_token = center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceDidWakeNotification),
            None,
            None,
            &did_wake_block,
        );
        std::mem::forget(did_wake_token);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn install_sleep_observers(
    _on_will_sleep: impl Fn() + Send + 'static,
    _on_did_wake: impl Fn() + Send + 'static,
) {
}

/// Whether the lid is currently closed with an external display attached
/// (clamshell mode), read from the IORegistry.
///
/// The mic health check calls this every 2s for the whole of a session when a
/// clamshell microphone is configured, so it reads the property in-process
/// rather than forking `ioreg`.
pub fn is_clamshell() -> bool {
    #[cfg(target_os = "macos")]
    {
        iokit::bool_property("IOPMrootDomain", "AppleClamshellState").unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Whether this Mac has a battery (i.e. is a laptop). Used to gate the
/// clamshell-microphone setting in the UI, which is meaningless on a desktop
/// Mac.
pub fn is_laptop() -> bool {
    #[cfg(target_os = "macos")]
    {
        iokit::service_exists("AppleSmartBattery")
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Minimal IOKit bindings. The framework is linked directly instead of pulling
/// a binding crate: four entry points, matching the `extern "C"` block already
/// used for the accessibility API in `permissions.rs`.
#[cfg(target_os = "macos")]
mod iokit {
    use std::ffi::{CString, c_char, c_void};
    use std::ptr::NonNull;

    use objc2_core_foundation::{CFBoolean, CFRetained, CFString, CFType};

    /// `io_object_t`, itself a `mach_port_t`.
    type IoObject = u32;

    /// `kIOMainPortDefault`: zero selects the default port for every entry
    /// point used here.
    const MAIN_PORT_DEFAULT: u32 = 0;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const c_char) -> *mut c_void;
        fn IOServiceGetMatchingService(main_port: u32, matching: *mut c_void) -> IoObject;
        fn IORegistryEntryCreateCFProperty(
            entry: IoObject,
            key: *const CFString,
            allocator: *const c_void,
            options: u32,
        ) -> *const CFType;
        fn IOObjectRelease(object: IoObject) -> i32;
    }

    /// The first service matching `class_name`, or `None` when the class is
    /// absent (e.g. `AppleSmartBattery` on a desktop Mac). The caller owns the
    /// returned object and must release it.
    fn matching_service(class_name: &str) -> Option<IoObject> {
        let name = CString::new(class_name).ok()?;
        // IOServiceGetMatchingService consumes the dictionary reference, so
        // this must not be released on the success path or the failure one.
        let matching = unsafe { IOServiceMatching(name.as_ptr()) };
        if matching.is_null() {
            return None;
        }
        let service = unsafe { IOServiceGetMatchingService(MAIN_PORT_DEFAULT, matching) };
        (service != 0).then_some(service)
    }

    pub(super) fn service_exists(class_name: &str) -> bool {
        match matching_service(class_name) {
            Some(service) => {
                unsafe { IOObjectRelease(service) };
                true
            }
            None => false,
        }
    }

    /// A boolean IORegistry property, or `None` when the service or the key is
    /// absent, or the value is not a boolean.
    pub(super) fn bool_property(class_name: &str, key: &str) -> Option<bool> {
        let service = matching_service(class_name)?;
        let cf_key = CFString::from_str(key);
        let raw = unsafe {
            IORegistryEntryCreateCFProperty(
                service,
                CFRetained::as_ptr(&cf_key).as_ptr(),
                std::ptr::null(),
                0,
            )
        };
        unsafe { IOObjectRelease(service) };
        // Create-rule: we own the returned value.
        let value = unsafe { CFRetained::from_raw(NonNull::new(raw.cast_mut())?) };
        value.downcast_ref::<CFBoolean>().map(CFBoolean::value)
    }
}

#[cfg(test)]
mod tests {
    use super::{is_clamshell, is_laptop};

    /// Both probes read live hardware state, so the value cannot be asserted
    /// here. What can be: they never panic, and repeated calls agree. A
    /// mismanaged CFRetained or a double IOObjectRelease would show up as a
    /// crash under these, which is the risk of hand-written IOKit bindings.
    #[test]
    fn probes_are_total_and_stable() {
        assert_eq!(is_clamshell(), is_clamshell());
        assert_eq!(is_laptop(), is_laptop());
    }

    /// The lid cannot be closed on a machine with no lid.
    #[test]
    fn clamshell_implies_laptop() {
        if is_clamshell() {
            assert!(is_laptop(), "clamshell reported on a machine with no battery");
        }
    }

    /// `is_clamshell` returning false is ambiguous: lid open, or a binding
    /// that silently reads nothing. Assert the property was actually found.
    /// Every Mac publishes AppleClamshellState on IOPMrootDomain.
    #[test]
    fn clamshell_property_is_actually_read() {
        assert!(
            super::iokit::bool_property("IOPMrootDomain", "AppleClamshellState").is_some(),
            "AppleClamshellState could not be read: the IOKit binding is broken"
        );
    }

    #[test]
    fn an_absent_service_class_is_reported_absent() {
        assert!(!super::iokit::service_exists("NoSuchServiceClassExists"));
    }
}

