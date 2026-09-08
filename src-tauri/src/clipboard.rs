use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AllocAnyThread, msg_send};
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSData, NSString};

use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

use crate::permissions;
use crate::settings::PasteMethod;

/// Shown when Accessibility is missing at paste time. Distinct from Enigo's
/// own init error so the UI can recognize it and offer the repair action
/// instead of just relaying a raw OS error string.
pub const ACCESSIBILITY_STALE_ERROR: &str = "Accessibility permission missing.";

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

const UTI_UTF8_PLAIN: &str = "public.utf8-plain-text";
const UTI_NSSTRING_PBOARD: &str = "NSStringPboardType";

/// Insert text into the active application after `delay_ms`, using either
/// clipboard+Cmd+V or simulated keystrokes (for apps that reject synthetic paste).
pub fn paste_text(text: &str, delay_ms: u64, method: PasteMethod) -> Result<(), String> {
    // Nothing to insert: leave the user's clipboard and their frontmost app
    // untouched rather than firing a ⌘V that pastes whatever was there.
    if is_blank(text) {
        return Ok(());
    }

    if !permissions::accessibility_granted() {
        // SOU-033: leave the transcription on the pasteboard so the user can
        // ⌘V. Do not snapshot/restore — restoring would wipe the only copy
        // they have. Accessibility still cannot restore rich pasteboard
        // contents because we never took a snapshot.
        return copy_instead_of_paste(text, copy_text);
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

/// Write `text` to the clipboard without pasting: no synthetic Cmd+V, no AX.
/// Backs the tray's "Copy Last Transcription" action, which exists partly to
/// work around SOU-010 (a paste landing with the wrong clipboard contents):
/// the user recovers by copying the same text a broken paste just wrote.
/// Cancelling the pending restore first is what makes that recovery work:
/// otherwise `restore_clipboard` would see its own text still on the
/// pasteboard and dutifully overwrite this copy with the pre-paste clipboard.
pub fn copy_text(text: &str) -> Result<(), String> {
    // Hold RESTORE across cancel + write: otherwise a restore that already
    // passed `take_if_current` can still `set_text` the pre-paste clipboard
    // after this copy returns. Same mutex as `restore_clipboard`.
    let mut burst = lock_restore();
    burst.cancel_pending_restore();

    let mut clipboard = Clipboard::new().map_err(|e| format!("Clipboard init: {e}"))?;
    clipboard
        .set_text(text)
        .map_err(|e| format!("Clipboard set: {e}"))?;

    if !wait_for_clipboard(&mut clipboard, text) {
        return Err("Clipboard write did not take effect.".to_string());
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
    let interval_ms = interval.as_millis().max(1);
    // Inclusive of the timeout boundary: 250 ms / 10 ms reads at 0, 10, …, 250.
    let attempts = timeout.as_millis() / interval_ms + 1;
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

/// Rust-owned pasteboard snapshot: each item is a list of (UTI, bytes).
/// Copying the bytes before `arboard.set_text` clears the board, then
/// writing fresh `NSPasteboardItem`s on restore. Live `NSPasteboardItem`
/// objects are not `Send` and cannot be reused after `clearContents`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PasteboardSnapshot(Vec<Vec<(String, Vec<u8>)>>);

impl PasteboardSnapshot {
    fn from_pasteboard() -> Option<Self> {
        let items = NSPasteboard::generalPasteboard().pasteboardItems()?;
        let mut out = Vec::new();
        for item in items.iter() {
            let mut flavors = Vec::new();
            for ty in item.types().iter() {
                let Some(data) = item.dataForType(&ty) else {
                    continue;
                };
                flavors.push((ty.to_string(), data.to_vec()));
            }
            if !flavors.is_empty() {
                out.push(flavors);
            }
        }
        if out.is_empty() {
            None
        } else {
            Some(Self(out))
        }
    }

    fn first_plain_text(&self) -> Option<String> {
        for flavors in &self.0 {
            for (uti, bytes) in flavors {
                if uti == UTI_UTF8_PLAIN || uti == UTI_NSSTRING_PBOARD {
                    return String::from_utf8(bytes.clone()).ok();
                }
            }
        }
        None
    }

    fn write(&self) -> bool {
        let pb = NSPasteboard::generalPasteboard();
        let mut writers = Vec::with_capacity(self.0.len());
        for flavors in &self.0 {
            // SAFETY: `init` is NSPasteboardItem's designated initializer and
            // returns a fully-formed item with no representations yet.
            let item: Retained<NSPasteboardItem> =
                unsafe { msg_send![NSPasteboardItem::alloc(), init] };
            for (uti, bytes) in flavors {
                let _ = item.setData_forType(&NSData::with_bytes(bytes), &NSString::from_str(uti));
            }
            writers.push(ProtocolObject::<dyn NSPasteboardWriting>::from_retained(
                item,
            ));
        }
        pb.clearContents();
        let array = NSArray::from_retained_slice(&writers);
        pb.writeObjects(&array)
    }
}

fn paste_via_cmd_v(text: &str, delay_ms: u64, enigo: &mut Enigo) -> Result<(), String> {
    // Snapshot before arboard clears the board. Register the restore
    // immediately so an overlapping paste during verify/⌘V keeps the
    // pre-burst clipboard, not the transcription.
    let previous = PasteboardSnapshot::from_pasteboard();
    let restore_gen = register_restore(previous);

    let mut clipboard = Clipboard::new().map_err(|e| format!("Clipboard init: {e}"))?;
    clipboard
        .set_text(text)
        .map_err(|e| format!("Clipboard set: {e}"))?;

    if !wait_for_clipboard(&mut clipboard, text) {
        tracing::warn!(
            timeout_ms = CLIPBOARD_VERIFY_TIMEOUT.as_millis(),
            "Clipboard did not serve the transcription in time; skipping paste"
        );
        spawn_restore(restore_gen, text.to_string());
        return Err(
            "Clipboard write did not take effect. Paste skipped so an older clipboard entry is not pasted instead."
                .to_string(),
        );
    }

    thread::sleep(Duration::from_millis(delay_ms));

    let paste_result = send_paste_keys(enigo);
    // The pasteboard already holds the transcription, whether ⌘V landed or
    // not. Restore either way so a key error does not leave it there.
    spawn_restore(restore_gen, text.to_string());
    paste_result
}

fn send_paste_keys(enigo: &mut Enigo) -> Result<(), String> {
    enigo
        .key(Key::Meta, Direction::Press)
        .map_err(|e| format!("Key press Meta: {e}"))?;
    enigo
        .key(Key::Unicode('v'), Direction::Click)
        .map_err(|e| format!("Key click V: {e}"))?;
    enigo
        .key(Key::Meta, Direction::Release)
        .map_err(|e| format!("Key release Meta: {e}"))
}

/// Snapshot of the clipboard from before the first paste in an overlapping
/// burst. Only the newest generation restores it; older restores no-op so a
/// second paste within `CLIPBOARD_RESTORE_DELAY` cannot write the first
/// transcription back as if it were the user's original contents.
///
/// Bytes are copied out of AppKit before the pasteboard is replaced. Restore
/// creates fresh `NSPasteboardItem`s rather than shipping live objects
/// across threads.
struct RestoreBurst {
    generation: u64,
    original: Option<PasteboardSnapshot>,
}

/// Accessibility is missing: write `text` for a manual ⌘V and return the
/// distinctive error the UI matches on. The copy error is appended so a
/// dead pasteboard is still visible, but the accessibility cause stays first.
fn copy_instead_of_paste(
    text: &str,
    copy: impl FnOnce(&str) -> Result<(), String>,
) -> Result<(), String> {
    match copy(text) {
        Ok(()) => Err(ACCESSIBILITY_STALE_ERROR.to_string()),
        Err(copy_err) => Err(format!("{ACCESSIBILITY_STALE_ERROR} ({copy_err})")),
    }
}

impl RestoreBurst {
    const fn new() -> Self {
        Self {
            generation: 0,
            original: None,
        }
    }

    fn begin(&mut self, previous: Option<PasteboardSnapshot>) -> (u64, bool) {
        self.generation = self.generation.wrapping_add(1);
        if self.original.is_none() {
            self.original = previous;
        }
        (self.generation, self.original.is_some())
    }

    /// `None` means a newer paste owns the restore. `Some(snapshot)` is the
    /// pre-burst clipboard, which may itself be `None`.
    fn take_if_current(&mut self, generation: u64) -> Option<Option<PasteboardSnapshot>> {
        if generation != self.generation {
            return None;
        }
        Some(self.original.take())
    }

    /// Disown any in-flight restore: bumping the generation makes its
    /// `take_if_current` check fail regardless of what text is on the
    /// clipboard when it wakes up, and clearing the snapshot leaves nothing
    /// to write back even if it somehow ran anyway.
    fn cancel_pending_restore(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.original = None;
    }
}

static RESTORE: Mutex<RestoreBurst> = Mutex::new(RestoreBurst::new());

fn lock_restore() -> std::sync::MutexGuard<'static, RestoreBurst> {
    RESTORE.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
fn cancel_pending_restore() {
    lock_restore().cancel_pending_restore();
}

fn register_restore(previous: Option<PasteboardSnapshot>) -> Option<u64> {
    let (generation, has_snapshot) = lock_restore().begin(previous);
    has_snapshot.then_some(generation)
}

/// The restore waits out `CLIPBOARD_RESTORE_DELAY`, and `paste_text` is a
/// synchronous Tauri command, so it runs on the main thread. Detach it rather
/// than freezing the UI for the whole wait; nothing downstream depends on it.
fn spawn_restore(generation: Option<u64>, ours: String) {
    let Some(generation) = generation else {
        return;
    };
    thread::spawn(move || restore_clipboard(generation, &ours));
}

/// Put the pre-burst clipboard back after a delay long enough for ⌘V to land,
/// and only if this generation is still current and the pasteboard still holds
/// `ours`. Anything else means another paste, app, or the user wrote in the
/// meantime. No-op when there was no previous snapshot.
///
/// Returns whether a restore was attempted (for unit tests; real AX is not
/// exercised in CI).
fn restore_clipboard(generation: u64, ours: &str) -> bool {
    thread::sleep(CLIPBOARD_RESTORE_DELAY);
    restore_clipboard_now(generation, ours)
}

/// Post-sleep half of the restore. Kept separate so tests can exercise the
/// lock window without waiting out `CLIPBOARD_RESTORE_DELAY`.
fn restore_clipboard_now(generation: u64, ours: &str) -> bool {
    // Stay locked through the pasteboard write so `copy_text` cannot land a
    // verified copy that we then overwrite with `previous`.
    let mut burst = lock_restore();
    let Some(Some(previous)) = burst.take_if_current(generation) else {
        return false;
    };
    let Ok(mut clipboard) = Clipboard::new() else {
        return false;
    };
    if !holds_text(clipboard.get_text().ok().as_deref(), ours) {
        tracing::warn!("Clipboard changed after paste; leaving the new contents in place");
        return false;
    }
    if previous.write() {
        return true;
    }
    tracing::warn!("pasteboard restore writeObjects failed");
    if let Some(text) = previous.first_plain_text() {
        let _ = clipboard.set_text(&text);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_snap(s: &str) -> Option<PasteboardSnapshot> {
        Some(PasteboardSnapshot(vec![vec![(
            UTI_UTF8_PLAIN.into(),
            s.as_bytes().to_vec(),
        )]]))
    }

    #[test]
    fn paste_method_variants_exist() {
        assert_ne!(PasteMethod::Clipboard, PasteMethod::Type);
        assert_ne!(PasteMethod::Ax, PasteMethod::Clipboard);
        assert_ne!(PasteMethod::Ax, PasteMethod::Type);
    }

    #[test]
    fn accessibility_stale_error_is_a_stable_prefix() {
        assert!(ACCESSIBILITY_STALE_ERROR.contains("Accessibility permission missing"));
        assert!(
            !ACCESSIBILITY_STALE_ERROR.contains("Settings >"),
            "itineraries belong in the UI button, not this error string"
        );
    }

    #[test]
    fn copy_instead_of_paste_leaves_the_transcription_and_errors() {
        let mut copied = None;
        let result = copy_instead_of_paste("hello", |text| {
            copied = Some(text.to_string());
            Ok(())
        });
        assert_eq!(result.unwrap_err(), ACCESSIBILITY_STALE_ERROR);
        assert_eq!(copied.as_deref(), Some("hello"));
    }

    #[test]
    fn copy_instead_of_paste_still_reports_accessibility_when_copy_fails() {
        let err = copy_instead_of_paste("hello", |_| Err("no pasteboard".into())).unwrap_err();
        assert_ne!(
            err.as_str(),
            ACCESSIBILITY_STALE_ERROR,
            "copy failure must not be the exact copied-for-⌘V signal"
        );
        assert!(err.starts_with(ACCESSIBILITY_STALE_ERROR), "{err}");
        assert!(err.contains("no pasteboard"), "{err}");
    }

    #[test]
    fn ax_set_applied_only_on_ok_true() {
        assert!(ax_set_applied(&Ok(true)));
        assert!(!ax_set_applied(&Ok(false)));
        assert!(!ax_set_applied(&Err("nope".into())));
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
            26
        );
        assert_eq!(
            verify_poll_attempts(Duration::from_millis(5), Duration::from_millis(10)),
            1
        );
        assert_eq!(
            verify_poll_attempts(Duration::from_millis(250), Duration::ZERO),
            251
        );
    }

    #[test]
    fn verify_timeout_stays_short_enough_to_not_stall_dictation() {
        assert!(CLIPBOARD_VERIFY_TIMEOUT <= Duration::from_millis(250));
        assert!(CLIPBOARD_VERIFY_INTERVAL < CLIPBOARD_VERIFY_TIMEOUT);
    }

    #[test]
    fn text_snapshots_compare_by_owned_bytes() {
        assert_eq!(text_snap("notes"), text_snap("notes"));
        assert_ne!(text_snap("notes"), text_snap("other"));
    }

    #[test]
    fn snapshot_keeps_two_flavors_on_one_item() {
        let snap = PasteboardSnapshot(vec![vec![
            (UTI_UTF8_PLAIN.into(), b"hi".to_vec()),
            ("public.rtf".into(), b"{\\rtf}".to_vec()),
        ]]);
        assert_eq!(snap.0[0].len(), 2);
        assert_eq!(snap.first_plain_text().as_deref(), Some("hi"));
    }

    #[test]
    fn overlapping_pastes_restore_the_pre_burst_clipboard() {
        let mut burst = RestoreBurst::new();
        let (first, _) = burst.begin(text_snap("notes"));
        let (second, has_snapshot) = burst.begin(text_snap("first transcription"));
        assert!(has_snapshot);
        assert!(
            burst.take_if_current(first).is_none(),
            "the older restore must not run once a newer paste owns the burst"
        );
        assert_eq!(
            burst.take_if_current(second),
            Some(text_snap("notes")),
            "the newest restore puts back what was there before either paste"
        );
    }

    #[test]
    fn a_failed_read_on_the_second_paste_still_keeps_the_original() {
        let mut burst = RestoreBurst::new();
        burst.begin(text_snap("notes"));
        let (second, has_snapshot) = burst.begin(None);
        assert!(has_snapshot);
        assert_eq!(burst.take_if_current(second), Some(text_snap("notes")));
    }

    #[test]
    fn first_paste_without_a_previous_value_has_nothing_to_restore() {
        let mut burst = RestoreBurst::new();
        let (generation, has_snapshot) = burst.begin(None);
        assert!(!has_snapshot);
        assert_eq!(burst.take_if_current(generation), Some(None));
    }

    #[test]
    fn cancel_pending_restore_blocks_a_scheduled_restore() {
        // The SOU-010 recovery scenario: a paste is pending a restore, and
        // the user copies before it fires. RestoreBurst has no notion of
        // "text" (that check lives in restore_clipboard against the live
        // pasteboard), so cancellation must be unconditional: it does not
        // matter here whether the copy used the identical string the paste
        // wrote.
        let mut burst = RestoreBurst::new();
        let (generation, has_snapshot) = burst.begin(text_snap("notes"));
        assert!(has_snapshot);

        burst.cancel_pending_restore();

        assert!(
            burst.take_if_current(generation).is_none(),
            "a copy must cancel the paste's pending restore, even with identical text"
        );
    }

    #[test]
    fn cancel_pending_restore_without_a_pending_paste_is_a_no_op() {
        let mut burst = RestoreBurst::new();
        burst.cancel_pending_restore();
        // The old generation (0) is stale either way.
        assert!(burst.take_if_current(0).is_none());
        // The new generation (1) matches, but there was never a snapshot to
        // restore, so restore_clipboard still has nothing to write back.
        assert_eq!(burst.take_if_current(1), Some(None));
    }

    #[test]
    fn a_new_paste_after_a_cancelled_restore_still_gets_its_own_snapshot() {
        let mut burst = RestoreBurst::new();
        let (first, _) = burst.begin(text_snap("notes"));
        burst.cancel_pending_restore();
        assert!(burst.take_if_current(first).is_none());

        let (second, has_snapshot) = burst.begin(text_snap("later"));
        assert!(has_snapshot);
        assert_eq!(burst.take_if_current(second), Some(text_snap("later")));
    }

    #[test]
    fn cancel_waits_out_a_restore_that_already_took_its_snapshot() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Barrier};
        use std::time::Duration;

        // The race CodeRabbit flagged: restore has already passed
        // take_if_current (and still holds RESTORE through the write).
        // cancel_pending_restore must not run until that lock is released,
        // or copy_text can land and then be overwritten by previous.
        let barrier = Arc::new(Barrier::new(2));
        let cancel_returned = Arc::new(AtomicBool::new(false));
        let cancel_handle = {
            let barrier = Arc::clone(&barrier);
            let cancel_returned = Arc::clone(&cancel_returned);
            thread::spawn(move || {
                barrier.wait();
                cancel_pending_restore();
                cancel_returned.store(true, Ordering::SeqCst);
            })
        };

        {
            let mut burst = lock_restore();
            // Reset + begin under the same lock as take_if_current so a
            // parallel spawn_restore cannot bump the generation in between.
            *burst = RestoreBurst::new();
            let generation = burst.begin(text_snap("notes")).0;
            assert!(burst.take_if_current(generation).unwrap().is_some());
            barrier.wait();
            thread::sleep(Duration::from_millis(50));
            assert!(
                !cancel_returned.load(Ordering::SeqCst),
                "cancel must block until the in-flight restore releases RESTORE"
            );
        }

        cancel_handle.join().expect("cancel thread");
        assert!(cancel_returned.load(Ordering::SeqCst));
        *lock_restore() = RestoreBurst::new();
    }
}
