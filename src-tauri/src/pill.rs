//! The floating recording pill — a small always-on-top window shown while
//! any recording (dictation or meeting) is active, so the user gets visual
//! feedback even when the main window is hidden. Visibility is driven from
//! the backend's state transitions (the single source of truth); content
//! and the stop action live in the pill's webview (`src/lib/pill/`).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, LogicalPosition, Manager};
use tauri_specta::Event;
use tracing::warn;

use crate::app_events::{PillHoldChanged, PillHoldKind};
use crate::state_machine::AppStateMachine;

/// Vertical offset below the menu bar.
const TOP_MARGIN: f64 = 40.0;

/// Minimum spacing between `DictationLiveText` emissions. Well under the
/// 5-10Hz cap so it reads as "live" without flooding the pill's IPC channel.
pub const LIVE_TEXT_MIN_INTERVAL: Duration = Duration::from_millis(120);

/// Tail length (characters) sent to the pill: enough for a compact 2-3 line
/// preview without shipping the whole running dictation on every update.
pub const LIVE_TEXT_MAX_CHARS: usize = 240;

/// Frontend-driven hold on pill visibility, independent of the state
/// machine (set/cleared via the `pill_hold` / `pill_release` commands).
/// `sync` clears it on every transition into a recording state, so a hold
/// whose release call was somehow lost (crash, error path) can never leave
/// a zombie pill once the user starts a new session.
static HOLD: Mutex<Option<PillHoldKind>> = Mutex::new(None);

fn set_hold_state(kind: PillHoldKind) {
    if let Ok(mut guard) = HOLD.lock() {
        *guard = Some(kind);
    }
}

/// Clears the hold, returning whether one was actually active (so callers
/// only emit a change event when something changed).
fn clear_hold_state() -> bool {
    HOLD.lock().map(|mut guard| guard.take().is_some()).unwrap_or(false)
}

fn is_held() -> bool {
    HOLD.lock().map(|guard| guard.is_some()).unwrap_or(false)
}

/// Engage a hold and notify the pill webview.
pub fn set_hold(app: &AppHandle, kind: PillHoldKind) {
    set_hold_state(kind);
    let _ = PillHoldChanged { kind: Some(kind) }.emit(app);
}

/// Release a hold. Safe to call with nothing held (e.g. paste succeeded
/// without dictation polish ever engaging a hold) — a no-op, no event.
pub fn clear_hold(app: &AppHandle) {
    if clear_hold_state() {
        let _ = PillHoldChanged { kind: None }.emit(app);
    }
}

/// Whether the pill should be visible given the current recording state and
/// hold. Pure so it's testable without a live window/AppHandle.
fn should_show_pill(recording: bool, held: bool) -> bool {
    recording || held
}

/// Show the pill while recording (or while held), hide it otherwise. Called
/// on every state transition; must never steal focus from the app the user
/// is dictating into (the window is configured with `focus: false` and we
/// only ever call `show`, never `set_focus`).
pub fn sync(app: &AppHandle, machine: &AppStateMachine) {
    let Some(pill) = app.get_webview_window("pill") else {
        return;
    };

    let recording = matches!(
        machine,
        AppStateMachine::RecordingDictation { .. } | AppStateMachine::RecordingMeeting { .. }
    );

    // A fresh recording starting is authoritative: any leftover hold from a
    // previous session (e.g. a release call that never landed) must not
    // keep blocking future hides.
    if recording {
        clear_hold(app);
    }

    let result = if should_show_pill(recording, is_held()) {
        position_top_center(&pill).and_then(|()| pill.show())
    } else {
        pill.hide()
    };
    if let Err(e) = result {
        warn!("Recording pill sync failed: {e}");
    }
}

/// Whether enough time has passed since the last live-text emission to send
/// another one. Pure decision — the `Instant::now()` call lives at the call
/// site so this is testable with fixed timestamps.
pub fn should_emit_live_text(last_emit: Option<Instant>, now: Instant, min_interval: Duration) -> bool {
    match last_emit {
        None => true,
        Some(last) => now.duration_since(last) >= min_interval,
    }
}

/// Last `max_chars` characters of `text` (UTF-8-safe: counts chars, not
/// bytes), so the pill shows a readable tail instead of the whole running
/// dictation.
pub fn live_text_tail(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    text.chars().skip(total - max_chars).collect()
}

fn position_top_center(pill: &tauri::WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = pill.primary_monitor()? else {
        return Ok(());
    };
    let scale = monitor.scale_factor();
    let monitor_width = monitor.size().to_logical::<f64>(scale).width;
    let pill_width = pill.outer_size()?.to_logical::<f64>(scale).width;
    pill.set_position(LogicalPosition::new(
        (monitor_width - pill_width) / 2.0,
        TOP_MARGIN,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_show_pill_when_recording_or_held() {
        assert!(should_show_pill(true, false));
        assert!(should_show_pill(false, true));
        assert!(should_show_pill(true, true));
        assert!(!should_show_pill(false, false));
    }

    #[test]
    fn should_emit_live_text_first_call_and_after_interval() {
        let interval = Duration::from_millis(100);
        let t0 = Instant::now();

        assert!(should_emit_live_text(None, t0, interval), "first sample always emits");
        assert!(
            !should_emit_live_text(Some(t0), t0 + Duration::from_millis(50), interval),
            "under the interval must be throttled"
        );
        assert!(
            should_emit_live_text(Some(t0), t0 + interval, interval),
            "exactly at the interval must emit"
        );
        assert!(
            should_emit_live_text(Some(t0), t0 + Duration::from_millis(150), interval),
            "past the interval must emit"
        );
    }

    #[test]
    fn live_text_tail_keeps_short_text_untouched() {
        assert_eq!(live_text_tail("hello world", 240), "hello world");
        assert_eq!(live_text_tail("", 240), "");
    }

    #[test]
    fn live_text_tail_truncates_to_the_last_n_chars() {
        let text = "0123456789";
        assert_eq!(live_text_tail(text, 4), "6789");
        assert_eq!(live_text_tail(text, 10), text);
        assert_eq!(live_text_tail(text, 0), "");
    }

    #[test]
    fn live_text_tail_is_utf8_safe() {
        // Multi-byte characters must not be split mid-codepoint.
        let text = "caf\u{e9} au lait \u{2615}"; // "café au lait ☕"
        let tail = live_text_tail(text, 5);
        assert_eq!(tail.chars().count(), 5);
        assert!(String::from_utf8(tail.into_bytes()).is_ok());
    }

    /// Exercises the full hold lifecycle against the shared module-level
    /// `HOLD` static in one test, so it can't race other tests touching it.
    #[test]
    fn hold_state_lifecycle_is_failure_safe() {
        assert!(!is_held(), "starts unheld");

        set_hold_state(PillHoldKind::Polishing);
        assert!(is_held());

        assert!(clear_hold_state(), "release reports it actually released something");
        assert!(!is_held());

        assert!(
            !clear_hold_state(),
            "releasing with nothing held is a safe no-op, never panics or reports a false release"
        );
        assert!(!is_held());
    }
}
