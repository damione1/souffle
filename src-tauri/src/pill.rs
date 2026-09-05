//! The floating recording pill — a small always-on-top window shown while
//! any recording (dictation or meeting) is active, so the user gets visual
//! feedback even when the main window is hidden. Visibility is driven from
//! the backend's state transitions (the single source of truth); content
//! and the stop action live in the pill's webview (`src/lib/pill/`).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};
use tauri_specta::Event;
use tracing::warn;

use crate::app_events::{PillHoldChanged, PillHoldKind};
use crate::db::Database;
use crate::settings::PILL_POSITION_KEY;
use crate::state::AppState;
use crate::state_machine::AppStateMachine;

/// Vertical offset below the menu bar.
const TOP_MARGIN: f64 = 40.0;

/// Compact dictation size — must match the "pill" window in tauri.conf.json
/// and `PILL_WIDTH`/`BASE_HEIGHT` in `PillApp.svelte`. Never derived from
/// `outer_size()`: after a scale/resolution change that physical size is
/// stale, and converting it with the new scale factor is what shrinks the
/// HUD into a tiny scrolling box (SOU-011).
const COMPACT_WIDTH: f64 = 280.0;
const COMPACT_HEIGHT: f64 = 64.0;

/// Last logical size the frontend asked for (`pill_resize`), or compact.
static LAST_SIZE: Mutex<(f64, f64)> = Mutex::new((COMPACT_WIDTH, COMPACT_HEIGHT));
/// User-dragged AppKit origin (bottom-left, global points). `None` = default
/// top-center on the active screen.
static CUSTOM_ORIGIN: Mutex<Option<(f64, f64)>> = Mutex::new(None);
/// Last origin we applied ourselves, so a follow-up `Moved` echo is ignored.
static LAST_APPLIED_ORIGIN: Mutex<(f64, f64)> = Mutex::new((0.0, 0.0));
static APPLYING_FRAME: AtomicBool = AtomicBool::new(false);
static PILL_HIDDEN: AtomicBool = AtomicBool::new(false);

/// Minimum spacing between `DictationLiveText` emissions. Well under the
/// 5-10Hz cap so it reads as "live" without flooding the pill's IPC channel.
pub const LIVE_TEXT_MIN_INTERVAL: Duration = Duration::from_millis(120);

/// Tail length (characters) sent to the pill: enough to fill the expanded
/// live-text preview (3-4 lines at the wider width) without shipping the
/// whole running dictation on every update.
pub const LIVE_TEXT_MAX_CHARS: usize = 360;

/// Frontend-driven hold on pill visibility, independent of the state
/// machine (set/cleared via the `pill_hold` / `pill_release` commands).
/// `sync` clears it only when *entering* a recording state, so a hold
/// whose release call was somehow lost (crash, error path) can never leave
/// a zombie pill once the user starts a new session — without also wiping
/// a polish hold that is engaged *before* stop, while still recording.
static HOLD: Mutex<Option<PillHoldKind>> = Mutex::new(None);

/// Last `recording` value observed by `sync`. Used so the leftover-hold
/// safety net only fires on a rising edge (idle → recording), not on every
/// sync while a session is already live.
static LAST_RECORDING: AtomicBool = AtomicBool::new(false);

fn set_hold_state(kind: PillHoldKind) {
    if let Ok(mut guard) = HOLD.lock() {
        *guard = Some(kind);
    }
}

/// Clears the hold, returning whether one was actually active (so callers
/// only emit a change event when something changed).
fn clear_hold_state() -> bool {
    HOLD.lock()
        .map(|mut guard| guard.take().is_some())
        .unwrap_or(false)
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

/// Whether the pill should be visible given the current recording state,
/// hold, and the user's hide preference. Pure so it's testable without a
/// live window/AppHandle.
fn should_show_pill(recording: bool, held: bool, hidden: bool) -> bool {
    !hidden && (recording || held)
}

pub fn set_hidden(hidden: bool) {
    PILL_HIDDEN.store(hidden, Ordering::SeqCst);
}

fn is_hidden() -> bool {
    PILL_HIDDEN.load(Ordering::SeqCst)
}

/// Drop a leftover hold only when a *new* recording starts. Polish holds
/// the pill *before* `stop_transcription` while the machine is still in a
/// recording state; clearing on `recording == true` would drop it on the
/// same `sync` that is supposed to keep the spinner up.
fn should_clear_hold_on_sync(was_recording: bool, now_recording: bool) -> bool {
    now_recording && !was_recording
}

/// Show the pill while recording (or while held), hide it otherwise. Called
/// on every state transition; must never steal focus from the app the user
/// is dictating into. We `orderFrontRegardless` a non-activating `NSPanel`
/// rather than Tauri's `show`/`set_focus`, which activate the app and kick
/// the user out of another app's fullscreen Space.
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
    let was_recording = LAST_RECORDING.swap(recording, Ordering::SeqCst);
    if should_clear_hold_on_sync(was_recording, recording) {
        clear_hold(app);
    }

    let result = if should_show_pill(recording, is_held(), is_hidden()) {
        apply_current_frame(&pill).and_then(|()| order_overlay(&pill, true))
    } else {
        persist_position(app);
        order_overlay(&pill, false)
    };
    if let Err(e) = result {
        warn!("Recording pill sync failed: {e}");
    }
}

/// Whether enough time has passed since the last live-text emission to send
/// another one. Pure decision — the `Instant::now()` call lives at the call
/// site so this is testable with fixed timestamps.
pub fn should_emit_live_text(
    last_emit: Option<Instant>,
    now: Instant,
    min_interval: Duration,
) -> bool {
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

/// Places the pill using the last requested logical size (compact until the
/// frontend has resized), on the active screen, clamped on-screen.
pub(crate) fn apply_current_frame(pill: &tauri::WebviewWindow) -> tauri::Result<()> {
    let (width, height) = last_size();
    set_frame(pill, width, height)
}

/// Lower/upper bounds on the pill's size, defensively clamped in
/// `set_frame_top_center` against whatever the frontend's live-text
/// measurement comes up with. The floor is the compact *meeting* HUD
/// (dot + stop); dictation compact is larger and is requested explicitly.
const MIN_WIDTH: f64 = 88.0;
const MAX_WIDTH: f64 = 600.0;
const MIN_HEIGHT: f64 = 40.0;
const MAX_HEIGHT: f64 = 260.0;

/// AppKit frame origin (bottom-left corner, global screen coordinates) that
/// keeps the pill's TOP edge pinned at `top_margin` below the top of
/// `screen` and horizontally centered on that screen. Pure so the anchoring
/// math is unit-testable without a live window.
fn frame_origin(
    screen_x: f64,
    screen_y: f64,
    screen_width: f64,
    screen_height: f64,
    width: f64,
    height: f64,
    top_margin: f64,
) -> (f64, f64) {
    let x = screen_x + (screen_width - width) / 2.0;
    let y = screen_y + screen_height - top_margin - height;
    (x, y)
}

/// Resizes and repositions the pill in a single native `setFrame:` call.
///
/// Default placement pins the top edge at `TOP_MARGIN` and centers
/// horizontally. A user-dragged origin is kept (top edge + x), then clamped
/// to the active screen so a saved position cannot land off-screen after a
/// resolution change.
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
pub(crate) fn set_frame_top_center(
    pill: &tauri::WebviewWindow,
    width: f64,
    height: f64,
) -> tauri::Result<()> {
    set_frame(pill, width, height)
}

fn last_size() -> (f64, f64) {
    LAST_SIZE
        .lock()
        .map(|guard| *guard)
        .unwrap_or((COMPACT_WIDTH, COMPACT_HEIGHT))
}

fn store_last_size(width: f64, height: f64) {
    if let Ok(mut guard) = LAST_SIZE.lock() {
        *guard = (width, height);
    }
}

fn custom_origin() -> Option<(f64, f64)> {
    CUSTOM_ORIGIN.lock().ok().and_then(|guard| *guard)
}

fn store_custom_origin(origin: Option<(f64, f64)>) {
    if let Ok(mut guard) = CUSTOM_ORIGIN.lock() {
        *guard = origin;
    }
}

/// Keep a rectangle of `width`×`height` fully inside `screen` when possible.
pub(crate) fn clamp_origin(
    screen_x: f64,
    screen_y: f64,
    screen_width: f64,
    screen_height: f64,
    width: f64,
    height: f64,
    x: f64,
    y: f64,
) -> (f64, f64) {
    let max_x = screen_x + screen_width - width;
    let max_y = screen_y + screen_height - height;
    let x = if max_x < screen_x {
        screen_x
    } else {
        x.clamp(screen_x, max_x)
    };
    let y = if max_y < screen_y {
        screen_y
    } else {
        y.clamp(screen_y, max_y)
    };
    (x, y)
}

fn origin_for_frame(
    screen_x: f64,
    screen_y: f64,
    screen_width: f64,
    screen_height: f64,
    width: f64,
    height: f64,
    previous_height: f64,
) -> (f64, f64) {
    if let Some((x, y)) = custom_origin() {
        let top = y + previous_height;
        clamp_origin(
            screen_x,
            screen_y,
            screen_width,
            screen_height,
            width,
            height,
            x,
            top - height,
        )
    } else {
        frame_origin(
            screen_x,
            screen_y,
            screen_width,
            screen_height,
            width,
            height,
            TOP_MARGIN,
        )
    }
}

fn set_frame(pill: &tauri::WebviewWindow, width: f64, height: f64) -> tauri::Result<()> {
    let width = width.clamp(MIN_WIDTH, MAX_WIDTH);
    let height = height.clamp(MIN_HEIGHT, MAX_HEIGHT);
    let previous_height = last_size().1;
    store_last_size(width, height);

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
        let overlay = configure_overlay_window(ns_window);
        let (screen_x, screen_y, screen_width, screen_height) = active_screen_frame();
        let (x, y) = origin_for_frame(
            screen_x,
            screen_y,
            screen_width,
            screen_height,
            width,
            height,
            previous_height,
        );
        let frame = objc2_foundation::NSRect {
            origin: objc2_foundation::NSPoint { x, y },
            size: objc2_foundation::NSSize { width, height },
        };
        APPLYING_FRAME.store(true, Ordering::SeqCst);
        overlay.setFrame_display(frame, true);
        if let Ok(mut guard) = LAST_APPLIED_ORIGIN.lock() {
            *guard = (x, y);
        }
        APPLYING_FRAME.store(false, Ordering::SeqCst);
    })
}

/// Record a user drag. Ignores the `Moved` echo from our own `setFrame`.
pub(crate) fn note_user_moved(pill: &tauri::WebviewWindow) {
    if APPLYING_FRAME.load(Ordering::SeqCst) {
        return;
    }
    let window = pill.clone();
    let _ = pill.run_on_main_thread(move || {
        if APPLYING_FRAME.load(Ordering::SeqCst) {
            return;
        }
        let Ok(ns_window_ptr) = window.ns_window() else {
            return;
        };
        // SAFETY: same contract as `set_frame` — pill NSWindow*, main thread.
        let ns_window: &objc2_app_kit::NSWindow = unsafe { &*ns_window_ptr.cast() };
        let origin = ns_window.frame().origin;
        let last = LAST_APPLIED_ORIGIN
            .lock()
            .map(|guard| *guard)
            .unwrap_or((origin.x, origin.y));
        if (origin.x - last.0).abs() < 2.0 && (origin.y - last.1).abs() < 2.0 {
            return;
        }
        store_custom_origin(Some((origin.x, origin.y)));
    });
}

pub(crate) fn restore_from_db(db: &Database, hidden: bool) {
    set_hidden(hidden);
    match db.get_setting(PILL_POSITION_KEY) {
        Ok(Some(raw)) => {
            if let Ok((x, y)) = serde_json::from_str::<(f64, f64)>(&raw) {
                store_custom_origin(Some((x, y)));
            }
        }
        Ok(None) => {}
        Err(e) => warn!("Failed to load pill position: {e}"),
    }
}

fn persist_position(app: &tauri::AppHandle) {
    let Some(origin) = custom_origin() else {
        return;
    };
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    match serde_json::to_string(&origin) {
        Ok(raw) => {
            if let Err(e) = state.db.set_setting(PILL_POSITION_KEY, &raw) {
                warn!("Failed to persist pill position: {e}");
            }
        }
        Err(e) => warn!("Failed to serialize pill position: {e}"),
    }
}

/// Show or hide the overlay without activating the app. Tauri's `show` /
/// `hide` go through NSWindow ordering that can fail to land on another
/// app's fullscreen Space; FluidVoice uses `orderFrontRegardless` /
/// `orderOut:` on a non-activating NSPanel instead.
fn order_overlay(pill: &tauri::WebviewWindow, visible: bool) -> tauri::Result<()> {
    let window = pill.clone();
    pill.run_on_main_thread(move || {
        let Ok(ns_window_ptr) = window.ns_window() else {
            warn!("Pill overlay: failed to get the native NSWindow handle");
            return;
        };
        // SAFETY: same contract as `set_frame_top_center` — pill NSWindow*,
        // main thread, window still alive.
        let ns_window: &objc2_app_kit::NSWindow = unsafe { &*ns_window_ptr.cast() };
        let overlay = configure_overlay_window(ns_window);
        if visible {
            overlay.orderFrontRegardless();
        } else {
            overlay.orderOut(None);
        }
    })
}

fn overlay_collection_behavior() -> objc2_app_kit::NSWindowCollectionBehavior {
    use objc2_app_kit::NSWindowCollectionBehavior;
    NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::FullScreenAuxiliary
        | NSWindowCollectionBehavior::IgnoresCycle
}

fn overlay_style_mask(
    current: objc2_app_kit::NSWindowStyleMask,
) -> objc2_app_kit::NSWindowStyleMask {
    current | objc2_app_kit::NSWindowStyleMask::NonactivatingPanel
}

/// Exclude the HUD from screenshots and screen sharing. The user still sees
/// it on their display; Zoom / Meet / ScreenCaptureKit do not. `None` is
/// the documented AppKit opt-out (`NSWindowSharingNone`).
fn overlay_sharing_type() -> objc2_app_kit::NSWindowSharingType {
    objc2_app_kit::NSWindowSharingType::None
}

/// Promote the Tauri webview's `NSWindow` to a non-activating `NSPanel`.
/// `FullScreenAuxiliary` is documented as an auxiliary-panel behavior —
/// setting it on a regular `NSWindow` (what we did previously) is ignored
/// by Mission Control, so the HUD stays stuck on the primary desktop Space.
///
/// Same `object_setClass` trick as tauri-nspanel / FluidVoice's native
/// `NSPanel(styleMask: [.borderless, .nonactivatingPanel])`.
fn configure_overlay_window(ns_window: &objc2_app_kit::NSWindow) -> &objc2_app_kit::NSWindow {
    use objc2::ClassType;
    use objc2::runtime::{AnyObject, NSObjectProtocol};
    use objc2_app_kit::{NSPanel, NSStatusWindowLevel, NSWindowAnimationBehavior};

    if !ns_window.isKindOfClass(NSPanel::class()) {
        // SAFETY: NSPanel is an NSWindow subclass; wry/tao windows are
        // NSWindow instances (or same-layout subclasses). Changing the isa
        // to NSPanel is the established overlay path (tauri-nspanel). The
        // webview hierarchy is untouched. ffi rather than AnyObject::set_class
        // because the latter debug-asserts equal instance_size and wry's
        // NSWindow subclass may not bitwise-match NSPanel.
        unsafe {
            let obj = std::ptr::from_ref(ns_window).cast::<AnyObject>().cast_mut();
            let _ = objc2::ffi::object_setClass(obj, NSPanel::class());
        }
    }

    // NSPanel-only bits — after the isa swap these selectors exist.
    // SAFETY: `ns_window` is now an NSPanel (or was already).
    let as_panel: &NSPanel = unsafe { &*std::ptr::from_ref(ns_window).cast::<NSPanel>() };
    as_panel.setFloatingPanel(true);
    as_panel.setBecomesKeyOnlyIfNeeded(true);
    as_panel.setWorksWhenModal(true);

    ns_window.setStyleMask(overlay_style_mask(ns_window.styleMask()));
    ns_window.setCollectionBehavior(overlay_collection_behavior());
    ns_window.setSharingType(overlay_sharing_type());
    ns_window.setLevel(NSStatusWindowLevel);
    ns_window.setHidesOnDeactivate(false);
    ns_window.setAnimationBehavior(NSWindowAnimationBehavior::None);
    ns_window
}

/// Screen that currently has keyboard focus (`NSScreen.main`), falling back
/// to the first screen. Fullscreen apps put the key window on that Space's
/// display, so this is where the HUD belongs — not always the primary
/// monitor.
fn active_screen_frame() -> (f64, f64, f64, f64) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSScreen;

    let Some(mtm) = MainThreadMarker::new() else {
        return (0.0, 0.0, 0.0, 0.0);
    };
    let screen = NSScreen::mainScreen(mtm).or_else(|| NSScreen::screens(mtm).firstObject());
    match screen {
        Some(screen) => {
            let frame = screen.frame();
            (
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height,
            )
        }
        None => (0.0, 0.0, 0.0, 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_show_pill_when_recording_or_held() {
        assert!(should_show_pill(true, false, false));
        assert!(should_show_pill(false, true, false));
        assert!(should_show_pill(true, true, false));
        assert!(!should_show_pill(false, false, false));
        assert!(
            !should_show_pill(true, true, true),
            "the hide option wins even while recording or held"
        );
    }

    #[test]
    fn should_clear_hold_only_when_entering_recording() {
        assert!(
            should_clear_hold_on_sync(false, true),
            "new session must drop a leftover hold"
        );
        assert!(
            !should_clear_hold_on_sync(true, true),
            "polish hold is set while still recording and must survive"
        );
        assert!(!should_clear_hold_on_sync(true, false));
        assert!(!should_clear_hold_on_sync(false, false));
    }

    #[test]
    fn overlay_collection_behavior_covers_fullscreen_spaces() {
        use objc2_app_kit::NSWindowCollectionBehavior;
        let behavior = overlay_collection_behavior();
        assert!(behavior.contains(NSWindowCollectionBehavior::CanJoinAllSpaces));
        assert!(behavior.contains(NSWindowCollectionBehavior::FullScreenAuxiliary));
        assert!(behavior.contains(NSWindowCollectionBehavior::IgnoresCycle));
    }

    #[test]
    fn overlay_style_mask_adds_nonactivating_panel() {
        use objc2_app_kit::NSWindowStyleMask;
        let mask = overlay_style_mask(NSWindowStyleMask::Borderless);
        assert!(mask.contains(NSWindowStyleMask::NonactivatingPanel));
    }

    #[test]
    fn overlay_is_excluded_from_screen_capture() {
        use objc2_app_kit::NSWindowSharingType;
        assert_eq!(overlay_sharing_type(), NSWindowSharingType::None);
    }

    #[test]
    fn should_emit_live_text_first_call_and_after_interval() {
        let interval = Duration::from_millis(100);
        let t0 = Instant::now();

        assert!(
            should_emit_live_text(None, t0, interval),
            "first sample always emits"
        );
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
        let (x, _y) = frame_origin(0.0, 0.0, 1920.0, 1080.0, 400.0, 100.0, 40.0);
        assert_eq!(x, (1920.0 - 400.0) / 2.0);
    }

    #[test]
    fn frame_origin_pins_the_top_edge_at_top_margin() {
        let monitor_height = 1080.0;
        let top_margin = 40.0;
        let (_x, y) = frame_origin(0.0, 0.0, 1920.0, monitor_height, 400.0, 100.0, top_margin);
        // AppKit's y is measured from the bottom, so the top edge sits at
        // `y + height`; that must land exactly `top_margin` below the
        // monitor's top (i.e. `monitor_height - top_margin`).
        assert_eq!(y + 100.0 + top_margin, monitor_height);
    }

    #[test]
    fn frame_origin_keeps_top_edge_fixed_as_height_grows() {
        let monitor_height = 1080.0;
        let top_margin = 40.0;
        let (_x, y_short) =
            frame_origin(0.0, 0.0, 1920.0, monitor_height, 400.0, 100.0, top_margin);
        let (_x, y_tall) = frame_origin(0.0, 0.0, 1920.0, monitor_height, 400.0, 180.0, top_margin);
        // Growing height by 80 must shift y down by exactly 80 to keep the
        // top edge (y + height) fixed.
        assert_eq!(y_short - y_tall, 80.0);
    }

    #[test]
    fn frame_origin_offsets_onto_a_secondary_screen() {
        let (x, y) = frame_origin(1920.0, 0.0, 1512.0, 982.0, 400.0, 100.0, 40.0);
        assert_eq!(x, 1920.0 + (1512.0 - 400.0) / 2.0);
        assert_eq!(y + 100.0 + 40.0, 982.0);
    }

    #[test]
    fn clamp_origin_pulls_a_saved_position_back_on_screen() {
        // Saved on a 1920×1080 display, then the display shrinks to 1280×800.
        let (x, y) = clamp_origin(0.0, 0.0, 1280.0, 800.0, 280.0, 64.0, 1700.0, 900.0);
        assert_eq!(x, 1280.0 - 280.0);
        assert_eq!(y, 800.0 - 64.0);
    }

    #[test]
    fn clamp_origin_leaves_an_on_screen_position_alone() {
        let (x, y) = clamp_origin(0.0, 0.0, 1920.0, 1080.0, 280.0, 64.0, 100.0, 200.0);
        assert_eq!((x, y), (100.0, 200.0));
    }

    #[test]
    fn clamp_origin_pins_when_the_window_is_larger_than_the_screen() {
        let (x, y) = clamp_origin(0.0, 0.0, 200.0, 50.0, 280.0, 64.0, 10.0, 10.0);
        assert_eq!((x, y), (0.0, 0.0));
    }

    #[test]
    fn compact_logical_size_is_the_configured_pill_window() {
        assert_eq!((COMPACT_WIDTH, COMPACT_HEIGHT), (280.0, 64.0));
    }

    #[test]
    fn restore_from_db_loads_a_saved_origin_and_hide_flag() {
        store_custom_origin(None);
        set_hidden(false);
        let (db, _dir) = crate::test_helpers::fixtures::test_db();
        db.set_setting(crate::settings::PILL_POSITION_KEY, "[100.5,200.25]")
            .unwrap();
        restore_from_db(&db, true);
        assert!(is_hidden());
        assert_eq!(custom_origin(), Some((100.5, 200.25)));
        set_hidden(false);
        store_custom_origin(None);
    }

    /// Exercises the full hold lifecycle against the shared module-level
    /// `HOLD` static in one test, so it can't race other tests touching it.
    #[test]
    fn hold_state_lifecycle_is_failure_safe() {
        assert!(!is_held(), "starts unheld");

        set_hold_state(PillHoldKind::Polishing);
        assert!(is_held());

        assert!(
            clear_hold_state(),
            "release reports it actually released something"
        );
        assert!(!is_held());

        assert!(
            !clear_hold_state(),
            "releasing with nothing held is a safe no-op, never panics or reports a false release"
        );
        assert!(!is_held());
    }
}
