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

/// The synthetic ⌘V is asynchronous: the target app reads the pasteboard from
/// its own run loop, and a busy app, an Electron one, or one that was just
/// launched can take a few hundred milliseconds to get there. Restoring
/// earlier races that read and the app pastes the previous clipboard contents
/// instead of the transcription.
const CLIPBOARD_RESTORE_DELAY: Duration = Duration::from_millis(400);

/// `set_text` can return before the pasteboard actually serves the new value,
/// so the write is read back before ⌘V is sent. Bounded low enough that a
/// failing pasteboard does not stall dictation.
const CLIPBOARD_VERIFY_TIMEOUT: Duration = Duration::from_millis(250);
const CLIPBOARD_VERIFY_INTERVAL: Duration = Duration::from_millis(10);

/// Insert text into the active application after `delay_ms`, using either
/// clipboard+Cmd+V or simulated keystrokes (for apps that reject synthetic paste).
pub fn paste_text(text: &str, delay_ms: u64, method: PasteMethod) -> Result<(), String> {
    // Nothing to insert: leave the user's clipboard and their frontmost app
    // untouched rather than firing a ⌘V that pastes whatever was there.
    if is_blank(text) {
        return Ok(());
    }

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

fn is_blank(text: &str) -> bool {
    text.trim().is_empty()
}

fn ax_set_applied(result: &Result<bool, String>) -> bool {
    matches!(result, Ok(true))
}

/// Whether the pasteboard currently serves exactly what we put there. Used
/// both to confirm our write before ⌘V and to decide whether the restore is
/// still ours to make.
fn holds_text(current: Option<&str>, expected: &str) -> bool {
    current == Some(expected)
}

fn verify_poll_attempts(timeout: Duration, interval: Duration) -> u32 {
    let attempts = timeout.as_millis() / interval.as_millis().max(1);
    u32::try_from(attempts).unwrap_or(u32::MAX).max(1)
}

/// Poll the pasteboard until it serves `text`, so ⌘V is never sent against a
/// write that has not landed.
fn wait_for_clipboard(clipboard: &mut Clipboard, text: &str) -> bool {
    let attempts = verify_poll_attempts(CLIPBOARD_VERIFY_TIMEOUT, CLIPBOARD_VERIFY_INTERVAL);
    for attempt in 0..attempts {
        if holds_text(clipboard.get_text().ok().as_deref(), text) {
            return true;
        }
        if attempt + 1 < attempts {
            thread::sleep(CLIPBOARD_VERIFY_INTERVAL);
        }
    }
    false
}

fn paste_via_cmd_v(text: &str, delay_ms: u64, enigo: &mut Enigo) -> Result<(), String> {
    let mut clipboard = Clipboard::new().map_err(|e| format!("Clipboard init: {e}"))?;
    let previous = clipboard.get_text().ok();
    clipboard
        .set_text(text)
        .map_err(|e| format!("Clipboard set: {e}"))?;

    if !wait_for_clipboard(&mut clipboard, text) {
        tracing::warn!(
            timeout_ms = CLIPBOARD_VERIFY_TIMEOUT.as_millis(),
            "Clipboard did not serve the transcription in time; skipping paste"
        );
        return Err(
            "Clipboard write did not take effect. Paste skipped so an older clipboard entry is not pasted instead."
                .to_string(),
        );
    }

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

    spawn_restore(previous, text.to_string());
    Ok(())
}

/// The restore waits out `CLIPBOARD_RESTORE_DELAY`, and `paste_text` is a
/// synchronous Tauri command, so it runs on the main thread. Detach it rather
/// than freezing the UI for the whole wait; nothing downstream depends on it.
fn spawn_restore(previous: Option<String>, ours: String) {
    if previous.is_none() {
        return;
    }
    thread::spawn(move || restore_clipboard(previous, &ours));
}

/// Put `previous` back after a delay long enough for ⌘V to land, and only if
/// the pasteboard still holds `ours`: anything else means another app or the
/// user wrote in the meantime and the restore would clobber it. No-op when
/// there was no previous text. Restore failures are ignored.
///
/// Returns whether a restore was attempted (for unit tests; real AX is not
/// exercised in CI).
fn restore_clipboard(previous: Option<String>, ours: &str) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    thread::sleep(CLIPBOARD_RESTORE_DELAY);
    let Ok(mut clipboard) = Clipboard::new() else {
        return false;
    };
    if !holds_text(clipboard.get_text().ok().as_deref(), ours) {
        tracing::warn!("Clipboard changed after paste; leaving the new contents in place");
        return false;
    }
    let _ = clipboard.set_text(&previous);
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
        assert!(!restore_clipboard(None, "text"));
    }

    #[test]
    fn spawn_restore_does_not_block_on_a_missing_previous() {
        let started = std::time::Instant::now();
        spawn_restore(None, "text".to_string());
        assert!(started.elapsed() < CLIPBOARD_RESTORE_DELAY);
    }

    #[test]
    fn clipboard_restore_delay_covers_slow_apps() {
        assert!(CLIPBOARD_RESTORE_DELAY >= Duration::from_millis(300));
        assert!(CLIPBOARD_RESTORE_DELAY <= Duration::from_millis(1000));
    }

    #[test]
    fn blank_text_is_not_pasted() {
        assert!(is_blank(""));
        assert!(is_blank("   "));
        assert!(is_blank("\n\t "));
        assert!(!is_blank("hi"));
        assert!(!is_blank(" hi "));
    }

    #[test]
    fn paste_text_is_a_no_op_for_blank_text() {
        // Runs without Accessibility in CI: the blank guard returns before
        // any permission check or Enigo init.
        assert_eq!(paste_text("   ", 0, PasteMethod::Clipboard), Ok(()));
        assert_eq!(paste_text("", 0, PasteMethod::Type), Ok(()));
    }

    #[test]
    fn holds_text_requires_an_exact_match() {
        assert!(holds_text(Some("hello"), "hello"));
        assert!(!holds_text(Some("hello "), "hello"));
        assert!(!holds_text(Some("something else"), "hello"));
        assert!(!holds_text(None, "hello"));
    }

    #[test]
    fn verify_poll_attempts_fit_the_timeout() {
        assert_eq!(
            verify_poll_attempts(Duration::from_millis(250), Duration::from_millis(10)),
            25
        );
        assert_eq!(
            verify_poll_attempts(Duration::from_millis(5), Duration::from_millis(10)),
            1
        );
        assert_eq!(
            verify_poll_attempts(Duration::from_millis(250), Duration::ZERO),
            250
        );
    }

    #[test]
    fn verify_timeout_stays_short_enough_to_not_stall_dictation() {
        assert!(CLIPBOARD_VERIFY_TIMEOUT <= Duration::from_millis(250));
        assert!(CLIPBOARD_VERIFY_INTERVAL < CLIPBOARD_VERIFY_TIMEOUT);
    }
}
