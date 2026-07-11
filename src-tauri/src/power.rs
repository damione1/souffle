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

use tracing::{info, warn};

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
/// (clamshell mode), read from the IORegistry. Shells out to `ioreg`, so
/// callers should only probe this at a coarse cadence (the mic health check
/// already runs on a multi-second timer).
pub fn is_clamshell() -> bool {
    #[cfg(target_os = "macos")]
    {
        match std::process::Command::new("ioreg")
            .args(["-r", "-k", "AppleClamshellState", "-d", "4"])
            .output()
        {
            Ok(output) => parse_clamshell(&String::from_utf8_lossy(&output.stdout)),
            Err(e) => {
                warn!("ioreg clamshell probe failed: {e}");
                false
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Pure parse of `ioreg -r -k AppleClamshellState -d 4` output: true only
/// when the key's line ends in `Yes`.
fn parse_clamshell(ioreg_output: &str) -> bool {
    ioreg_output
        .lines()
        .find(|line| line.contains("\"AppleClamshellState\""))
        .is_some_and(|line| line.trim_end().ends_with("Yes"))
}

/// Whether this Mac has a battery (i.e. is a laptop), read via `pmset`. Used
/// to gate the clamshell-microphone setting in the UI — it's meaningless on
/// a desktop Mac.
pub fn is_laptop() -> bool {
    #[cfg(target_os = "macos")]
    {
        match std::process::Command::new("pmset").args(["-g", "batt"]).output() {
            Ok(output) => parse_is_laptop(&String::from_utf8_lossy(&output.stdout)),
            Err(e) => {
                warn!("pmset laptop probe failed: {e}");
                false
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Pure parse of `pmset -g batt` output: a battery-equipped Mac reports an
/// `InternalBattery` power source.
fn parse_is_laptop(pmset_output: &str) -> bool {
    pmset_output.contains("InternalBattery")
}

#[cfg(test)]
mod tests {
    use super::{parse_clamshell, parse_is_laptop};

    #[test]
    fn clamshell_yes_is_detected() {
        let output = "+-o AppleClamshellState  <class ...>\n    | {\n    |   \"AppleClamshellState\" = Yes\n    | }\n";
        assert!(parse_clamshell(output));
    }

    #[test]
    fn clamshell_no_is_not_detected() {
        let output = "    | {\n    |   \"AppleClamshellState\" = No\n    | }\n";
        assert!(!parse_clamshell(output));
    }

    #[test]
    fn clamshell_garbage_output_defaults_false() {
        assert!(!parse_clamshell(""));
        assert!(!parse_clamshell("not even close to ioreg output"));
        assert!(!parse_clamshell("\"SomeOtherKey\" = Yes"));
    }

    #[test]
    fn laptop_detected_from_internal_battery() {
        let output = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=123)\t100%; charged; 0:00 remaining present: true\n";
        assert!(parse_is_laptop(output));
    }

    #[test]
    fn desktop_has_no_internal_battery() {
        let output = "Now drawing from 'AC Power'\n";
        assert!(!parse_is_laptop(output));
    }

    #[test]
    fn laptop_garbage_output_defaults_false() {
        assert!(!parse_is_laptop(""));
        assert!(!parse_is_laptop("nonsense"));
    }
}
