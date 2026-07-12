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

/// Tail length (characters) sent to the pill: enough to fill the expanded
/// live-text preview (3-4 lines at the wider width) without shipping the
/// whole running dictation on every update.
pub const LIVE_TEXT_MAX_CHARS: usize = 360;

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

/// Recenters the pill horizontally below the menu bar. `pub(crate)` so
/// `sync` can call it when showing the pill in its compact state.
pub(crate) fn position_top_center(pill: &tauri::WebviewWindow) -> tauri::Result<()> {
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

/// Lower/upper bounds on the pill's size, defensively clamped in
/// `set_frame_top_center` against whatever the frontend's live-text
/// measurement comes up with.
const MIN_WIDTH: f64 = 280.0;
const MAX_WIDTH: f64 = 600.0;
const MIN_HEIGHT: f64 = 64.0;
const MAX_HEIGHT: f64 = 260.0;

/// AppKit frame origin (bottom-left corner, in the primary screen's
/// coordinate space) that keeps the pill's TOP edge pinned at `top_margin`
/// below the menu bar and horizontally centered. Pure so the anchoring math
/// is unit-testable without a live window.
fn frame_origin(monitor_width: f64, monitor_height: f64, width: f64, height: f64, top_margin: f64) -> (f64, f64) {
    let x = (monitor_width - width) / 2.0;
    let y = monitor_height - top_margin - height;
    (x, y)
}

/// Resizes and repositions the pill in a single native `setFrame:` call, so
/// the top edge stays pinned at `TOP_MARGIN` and the window stays centered
/// as its height grows with the live transcript.
///
/// This bypasses tao's `set_inner_size` (AppKit's `setContentSize:` anchors
/// the window's BOTTOM-left corner, so growing the height pushes the top
/// edge into the menu bar until AppKit's `constrainFrameRect` clamps it) and
/// bypasses the two-step JS resize-then-recenter dance, which raced because
/// tao dispatches the resize asynchronously.
///
/// The frame is applied without animation (`setFrame:display:`, not
/// `setFrame:display:animate:`). `animate:YES` runs synchronously on the
/// main thread via a nested run loop (`NSAnimation`) until the animation
/// finishes, and it does not bail out early if the window is ordered out
/// mid-animation. At end-of-dictation the pill resizes back to compact at
/// the same moment the backend hides the window, so an animated call here
/// can spin forever and deadlock the main thread.
pub(crate) fn set_frame_top_center(pill: &tauri::WebviewWindow, width: f64, height: f64) -> tauri::Result<()> {
    let width = width.clamp(MIN_WIDTH, MAX_WIDTH);
    let height = height.clamp(MIN_HEIGHT, MAX_HEIGHT);

    let Some(monitor) = pill.primary_monitor()? else {
        return Ok(());
    };
    let scale = monitor.scale_factor();
    let monitor_size = monitor.size().to_logical::<f64>(scale);
    let (x, y) = frame_origin(monitor_size.width, monitor_size.height, width, height, TOP_MARGIN);

    let window = pill.clone();
    pill.run_on_main_thread(move || {
        let Ok(ns_window_ptr) = window.ns_window() else {
            warn!("Pill resize: failed to get the native NSWindow handle");
            return;
        };
        // SAFETY: `ns_window_ptr` comes from `WebviewWindow::ns_window`, which
        // returns the pill's own NSWindow* for as long as the window is
        // alive; we're on the main thread (required for AppKit calls) inside
        // this `run_on_main_thread` closure.
        let ns_window: &objc2_app_kit::NSWindow = unsafe { &*ns_window_ptr.cast() };
        let frame = objc2_foundation::NSRect {
            origin: objc2_foundation::NSPoint { x, y },
            size: objc2_foundation::NSSize { width, height },
        };
        ns_window.setFrame_display(frame, true);
    })
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

    #[test]
    fn frame_origin_centers_horizontally() {
        let (x, _y) = frame_origin(1920.0, 1080.0, 400.0, 100.0, 40.0);
        assert_eq!(x, (1920.0 - 400.0) / 2.0);
    }

    #[test]
    fn frame_origin_pins_the_top_edge_at_top_margin() {
        let monitor_height = 1080.0;
        let top_margin = 40.0;
        let (_x, y) = frame_origin(1920.0, monitor_height, 400.0, 100.0, top_margin);
        // AppKit's y is measured from the bottom, so the top edge sits at
        // `y + height`; that must land exactly `top_margin` below the
        // monitor's top (i.e. `monitor_height - top_margin`).
        assert_eq!(y + 100.0 + top_margin, monitor_height);
    }

    #[test]
    fn frame_origin_keeps_top_edge_fixed_as_height_grows() {
        let monitor_height = 1080.0;
        let top_margin = 40.0;
        let (_x, y_short) = frame_origin(1920.0, monitor_height, 400.0, 100.0, top_margin);
        let (_x, y_tall) = frame_origin(1920.0, monitor_height, 400.0, 180.0, top_margin);
        // Growing height by 80 must shift y down by exactly 80 to keep the
        // top edge (y + height) fixed.
        assert_eq!(y_short - y_tall, 80.0);
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
