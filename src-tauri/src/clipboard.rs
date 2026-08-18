use std::thread;
use std::time::Duration;

use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

use crate::permissions;
use crate::settings::PasteMethod;

/// Shown when Accessibility is missing at paste time. Distinct from Enigo's
/// own init error so the UI can recognize it and offer the repair action
/// instead of just relaying a raw OS error string.
pub const ACCESSIBILITY_STALE_ERROR: &str = "Accessibility permission missing. If Souffle is already listed and checked in System Settings > Privacy & Security > Accessibility, this is usually a stale entry left by an app update: remove Souffle with the minus button and re-add it, or use Repair permission in Souffle's Settings > Advanced > Permissions.";

/// Wait for ⌘V to land in the target app before putting the previous
/// clipboard contents back.
const CLIPBOARD_RESTORE_DELAY: Duration = Duration::from_millis(75);

/// Insert text into the active application after `delay_ms`, using either
/// clipboard+Cmd+V or simulated keystrokes (for apps that reject synthetic paste).
pub fn paste_text(text: &str, delay_ms: u64, method: PasteMethod) -> Result<(), String> {
    if !permissions::accessibility_granted() {
        return Err(ACCESSIBILITY_STALE_ERROR.to_string());
    }

    // Never let a background paste pop the OS permission pane on its own;
    // the accessibility_granted() check above already handles the
    // user-facing prompt path via the permissions/onboarding flow.
    let settings = Settings {
        open_prompt_to_get_permissions: false,
        ..Default::default()
    };
    let mut enigo = Enigo::new(&settings).map_err(|e| format!("Enigo init: {e}"))?;

    match method {
        PasteMethod::Ax => {
            thread::sleep(Duration::from_millis(delay_ms));
            if ax_set_applied(&crate::ax_text::set_selected_text(text)) {
                return Ok(());
            }
            paste_via_cmd_v(text, 0, &mut enigo)?;
        }
        PasteMethod::Clipboard => {
            paste_via_cmd_v(text, delay_ms, &mut enigo)?;
        }
        PasteMethod::Type => {
            thread::sleep(Duration::from_millis(delay_ms));

            enigo
                .text(text)
                .map_err(|e| format!("Simulated typing: {e}"))?;
        }
    }

    Ok(())
}

fn ax_set_applied(result: &Result<bool, String>) -> bool {
    matches!(result, Ok(true))
}

fn paste_via_cmd_v(text: &str, delay_ms: u64, enigo: &mut Enigo) -> Result<(), String> {
    let mut clipboard = Clipboard::new().map_err(|e| format!("Clipboard init: {e}"))?;
    let previous = clipboard.get_text().ok();
    clipboard
        .set_text(text)
        .map_err(|e| format!("Clipboard set: {e}"))?;

    thread::sleep(Duration::from_millis(delay_ms));

    enigo
        .key(Key::Meta, Direction::Press)
        .map_err(|e| format!("Key press Meta: {e}"))?;
    enigo
        .key(Key::Unicode('v'), Direction::Click)
        .map_err(|e| format!("Key click V: {e}"))?;
    enigo
        .key(Key::Meta, Direction::Release)
        .map_err(|e| format!("Key release Meta: {e}"))?;

    restore_clipboard(previous);
    Ok(())
}

/// Put `previous` back after a short delay so ⌘V can complete. No-op when
/// there was no previous text. Restore failures are ignored.
///
/// Returns whether a restore was attempted (for unit tests; real AX is not
/// exercised in CI).
fn restore_clipboard(previous: Option<String>) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    thread::sleep(CLIPBOARD_RESTORE_DELAY);
    if let Ok(mut clipboard) = Clipboard::new() {
        let _ = clipboard.set_text(&previous);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_method_variants_exist() {
        assert_ne!(PasteMethod::Clipboard, PasteMethod::Type);
        assert_ne!(PasteMethod::Ax, PasteMethod::Clipboard);
        assert_ne!(PasteMethod::Ax, PasteMethod::Type);
    }

    #[test]
    fn accessibility_stale_error_points_to_repair() {
        assert!(ACCESSIBILITY_STALE_ERROR.contains("Accessibility"));
        assert!(ACCESSIBILITY_STALE_ERROR.contains("Repair permission"));
    }

    #[test]
    fn ax_set_applied_only_on_ok_true() {
        assert!(ax_set_applied(&Ok(true)));
        assert!(!ax_set_applied(&Ok(false)));
        assert!(!ax_set_applied(&Err("nope".into())));
    }

    #[test]
    fn restore_clipboard_skips_when_no_previous() {
        assert!(!restore_clipboard(None));
    }

    #[test]
    fn clipboard_restore_delay_allows_paste_to_complete() {
        assert!(CLIPBOARD_RESTORE_DELAY >= Duration::from_millis(50));
        assert!(CLIPBOARD_RESTORE_DELAY <= Duration::from_millis(100));
    }
}
