//! The floating recording pill — a native NSPanel (SOU-051) shown while any
//! recording (dictation or meeting) is active, giving the user visual feedback
//! even when the main window is hidden.
//!
//! The panel is owned entirely by Swift (`pill_panel.swift`). This module
//! holds the Rust-side state (HOLD, HIDDEN, position persistence) and drives
//! the Swift layer via the C bridge (`pill_bridge.h`).
//!
//! Key design invariants:
//! - Visibility is driven by `pill::sync` — the single source of truth is the
//!   `AppStateMachine` + the HOLD flag + the user's hide preference.
//! - Sizing and positioning are computed inside Swift; Rust never calls
//!   `setFrame:` — it only tells Swift what mode we're in.
//! - The stop button invokes a C callback registered at startup. Dictation
//!   stop is `DictationStopRequested` (stop-only, not a toggle — SOU-044/046);
//!   the main-window controller then runs `stop_transcription` + polish/paste.
//!   Meetings use `MeetingStopRequested`, same path as the tray.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};
use tauri_specta::Event;
use tracing::warn;

use crate::app_events::{
    DictationStopRequested, MeetingStopRequested, PillHoldChanged, PillHoldKind,
};
use crate::db::Database;
use crate::settings::PILL_POSITION_KEY;
use crate::state::AppState;
use crate::state_machine::AppStateMachine;

// ---------------------------------------------------------------------------
// FFI (pill_bridge.h / pill_panel.swift)
// ---------------------------------------------------------------------------

/// Recording mode passed to the Swift panel (mirrors `PillMode` in Swift).
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PillPanelMode {
    Dictation = 0,
    Meeting = 1,
    Polishing = 2,
}

type PillStopCallback = unsafe extern "C" fn(recording_mode: i32);

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
unsafe extern "C" {
    fn pill_panel_create();
    fn pill_panel_set_visible(visible: i32);
    fn pill_panel_set_mode(
        mode: i32,
        title: *const std::ffi::c_char,
        stop_label: *const std::ffi::c_char,
        a11y_label: *const std::ffi::c_char,
    );
    fn pill_panel_set_live_text(text: *const std::ffi::c_char);
    fn pill_panel_push_rms(level: f32);
    fn pill_panel_restore_origin(x: f64, y: f64);
    fn pill_panel_get_origin(out_x: *mut f64, out_y: *mut f64) -> i32;
    fn pill_panel_set_stop_callback(callback: Option<PillStopCallback>);
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
unsafe fn pill_panel_create() {}
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
unsafe fn pill_panel_set_visible(_visible: i32) {}
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
unsafe fn pill_panel_set_mode(
    _mode: i32,
    _title: *const std::ffi::c_char,
    _stop_label: *const std::ffi::c_char,
    _a11y_label: *const std::ffi::c_char,
) {
}
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
unsafe fn pill_panel_set_live_text(_text: *const std::ffi::c_char) {}
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
unsafe fn pill_panel_push_rms(_level: f32) {}
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
unsafe fn pill_panel_restore_origin(_x: f64, _y: f64) {}
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
unsafe fn pill_panel_get_origin(_out_x: *mut f64, _out_y: *mut f64) -> i32 {
    0
}
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
unsafe fn pill_panel_set_stop_callback(_callback: Option<PillStopCallback>) {}

// ---------------------------------------------------------------------------
// Module-level state
// ---------------------------------------------------------------------------

/// Frontend-driven hold on pill visibility, independent of the state machine
/// (set/cleared via the `pill_hold` / `pill_release` commands).
/// `sync` clears it only when *entering* a recording state, so a hold whose
/// release call was somehow lost can never leave a zombie pill once the user
/// starts a new session.
static HOLD: Mutex<Option<PillHoldKind>> = Mutex::new(None);

/// Last `recording` value observed by `sync`. Used so the leftover-hold
/// safety net only fires on a rising edge (idle → recording).
static LAST_RECORDING: AtomicBool = AtomicBool::new(false);

static PILL_HIDDEN: AtomicBool = AtomicBool::new(false);

/// Restored / last-known origin, kept in Rust so `restore_from_db` is
/// testable without AppKit.
static CUSTOM_ORIGIN: Mutex<Option<(f64, f64)>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Live-text constants (public — used by commands::transcription)
// ---------------------------------------------------------------------------

/// Minimum spacing between `DictationLiveText` emissions.
pub const LIVE_TEXT_MIN_INTERVAL: Duration = Duration::from_millis(120);

/// Tail length (characters) sent to the pill.
pub const LIVE_TEXT_MAX_CHARS: usize = 360;

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Create the native panel and install the stop callback.
/// Must be called from the Tauri setup closure (main thread).
pub fn create_panel(app: &AppHandle) {
    // SAFETY: Swift hops to the main thread internally.
    unsafe { pill_panel_create() };
    install_stop_callback(app.clone());
}

fn install_stop_callback(app: AppHandle) {
    STOP_APP_HANDLE.set(std::sync::Mutex::new(Some(app))).ok();

    unsafe extern "C" fn on_stop(recording_mode: i32) {
        let Some(guard) = STOP_APP_HANDLE.get() else {
            return;
        };
        let Ok(guard) = guard.lock() else {
            return;
        };
        let Some(app) = guard.as_ref() else {
            return;
        };

        if recording_mode == PillPanelMode::Meeting as i32 {
            let _ = MeetingStopRequested.emit(app);
        } else {
            // Stop-only. Never ShortcutToggle: that can start a new dictation
            // or (historically) take down a meeting (SOU-044 / SOU-046).
            let _ = DictationStopRequested.emit(app);
        }
    }

    // SAFETY: `on_stop` is a plain C function pointer; Swift may call it
    // from the main thread. AppHandle is Send + Sync.
    unsafe { pill_panel_set_stop_callback(Some(on_stop)) };
}

static STOP_APP_HANDLE: std::sync::OnceLock<std::sync::Mutex<Option<AppHandle>>> =
    std::sync::OnceLock::new();

// ---------------------------------------------------------------------------
// Visibility helpers
// ---------------------------------------------------------------------------

pub fn set_hidden(hidden: bool) {
    PILL_HIDDEN.store(hidden, Ordering::SeqCst);
}

fn is_hidden() -> bool {
    PILL_HIDDEN.load(Ordering::SeqCst)
}

fn set_hold_state(kind: PillHoldKind) {
    if let Ok(mut guard) = HOLD.lock() {
        *guard = Some(kind);
    }
}

fn clear_hold_state() -> bool {
    HOLD.lock()
        .map(|mut guard| guard.take().is_some())
        .unwrap_or(false)
}

fn is_held() -> bool {
    HOLD.lock().map(|guard| guard.is_some()).unwrap_or(false)
}

fn should_show_pill(recording: bool, held: bool, hidden: bool) -> bool {
    !hidden && (recording || held)
}

fn should_clear_hold_on_sync(was_recording: bool, now_recording: bool) -> bool {
    now_recording && !was_recording
}

fn store_custom_origin(origin: Option<(f64, f64)>) {
    if let Ok(mut guard) = CUSTOM_ORIGIN.lock() {
        *guard = origin;
    }
}

#[cfg(test)]
fn custom_origin() -> Option<(f64, f64)> {
    CUSTOM_ORIGIN.lock().ok().and_then(|guard| *guard)
}

fn locale_is_french(app: &AppHandle) -> bool {
    app.try_state::<AppState>()
        .and_then(|state| crate::settings::AppSettings::load(&state.db).ok())
        .map(|settings| settings.locale.starts_with("fr"))
        .unwrap_or(false)
}

fn mode_title(mode: PillPanelMode, fr: bool) -> &'static str {
    match (mode, fr) {
        (PillPanelMode::Dictation, false) => "Dictating",
        (PillPanelMode::Dictation, true) => "Dictée",
        (PillPanelMode::Meeting, _) => "",
        (PillPanelMode::Polishing, false) => "Reformulating…",
        (PillPanelMode::Polishing, true) => "Reformulation…",
    }
}

fn stop_label(fr: bool) -> &'static str {
    if fr {
        "Arrêter l'enregistrement"
    } else {
        "Stop recording"
    }
}

fn a11y_label(mode: PillPanelMode, fr: bool) -> &'static str {
    match (mode, fr) {
        (PillPanelMode::Dictation, false) => "Dictation in progress",
        (PillPanelMode::Dictation, true) => "Dictée en cours",
        (PillPanelMode::Meeting, false) => "Meeting recording in progress",
        (PillPanelMode::Meeting, true) => "Enregistrement de meeting en cours",
        (PillPanelMode::Polishing, false) => "Reformulating",
        (PillPanelMode::Polishing, true) => "Reformulation",
    }
}

fn to_cstring(s: &str) -> Option<std::ffi::CString> {
    std::ffi::CString::new(s).ok()
}

fn apply_mode(mode: PillPanelMode, fr: bool) {
    let Some(title) = to_cstring(mode_title(mode, fr)) else {
        return;
    };
    let Some(stop) = to_cstring(stop_label(fr)) else {
        return;
    };
    let Some(a11y) = to_cstring(a11y_label(mode, fr)) else {
        return;
    };
    unsafe {
        pill_panel_set_mode(mode as i32, title.as_ptr(), stop.as_ptr(), a11y.as_ptr());
    }
}

// ---------------------------------------------------------------------------
// Public hold API
// ---------------------------------------------------------------------------

pub fn set_hold(app: &AppHandle, kind: PillHoldKind) {
    set_hold_state(kind);
    let _ = PillHoldChanged { kind: Some(kind) }.emit(app);
}

pub fn clear_hold(app: &AppHandle) {
    if clear_hold_state() {
        let _ = PillHoldChanged { kind: None }.emit(app);
    }
}

// ---------------------------------------------------------------------------
// Main sync entry point
// ---------------------------------------------------------------------------

/// Show/hide the pill and update mode. Called on every state transition.
pub fn sync(app: &AppHandle, machine: &AppStateMachine) {
    let recording = matches!(
        machine,
        AppStateMachine::RecordingDictation { .. } | AppStateMachine::RecordingMeeting { .. }
    );

    let was_recording = LAST_RECORDING.swap(recording, Ordering::SeqCst);
    if should_clear_hold_on_sync(was_recording, recording) {
        clear_hold(app);
    }

    let show = should_show_pill(recording, is_held(), is_hidden());
    let fr = locale_is_french(app);

    let mode = if is_held() {
        PillPanelMode::Polishing
    } else {
        match machine {
            AppStateMachine::RecordingMeeting { .. } => PillPanelMode::Meeting,
            AppStateMachine::Stopping { was_recording, .. } => {
                if matches!(
                    was_recording,
                    crate::state_machine::RecordingKind::Meeting { .. }
                ) {
                    PillPanelMode::Meeting
                } else {
                    PillPanelMode::Dictation
                }
            }
            _ => PillPanelMode::Dictation,
        }
    };

    apply_mode(mode, fr);

    if !show {
        persist_position(app);
        if let Some(empty) = to_cstring("") {
            unsafe { pill_panel_set_live_text(empty.as_ptr()) };
        }
    }

    unsafe { pill_panel_set_visible(if show { 1 } else { 0 }) };
}

// ---------------------------------------------------------------------------
// Live text / RMS API
// ---------------------------------------------------------------------------

/// Push a new live-text tail to the Swift panel. Thread-safe; Swift hops
/// to the main thread internally.
pub fn push_live_text(text: &str) {
    let Some(cstr) = to_cstring(text) else {
        return;
    };
    unsafe { pill_panel_set_live_text(cstr.as_ptr()) };
}

/// Push a new RMS level (0.0–1.0) for the waveform animation.
pub fn push_rms(level: f32) {
    unsafe { pill_panel_push_rms(level) };
}

/// Whether enough time has passed since the last live-text emission.
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

/// Last `max_chars` characters of `text` (UTF-8-safe).
pub fn live_text_tail(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    text.chars().skip(total - max_chars).collect()
}

// ---------------------------------------------------------------------------
// Position persistence
// ---------------------------------------------------------------------------

pub(crate) fn restore_from_db(db: &Database, hidden: bool) {
    set_hidden(hidden);
    match db.get_setting(PILL_POSITION_KEY) {
        Ok(Some(raw)) => {
            if let Ok((x, y)) = serde_json::from_str::<(f64, f64)>(&raw) {
                store_custom_origin(Some((x, y)));
                unsafe { pill_panel_restore_origin(x, y) };
            }
        }
        Ok(None) => {}
        Err(e) => warn!("Failed to load pill position: {e}"),
    }
}

fn persist_position(app: &tauri::AppHandle) {
    let mut x: f64 = 0.0;
    let mut y: f64 = 0.0;
    let has_origin = unsafe { pill_panel_get_origin(&mut x, &mut y) } != 0;
    if !has_origin {
        return;
    }
    store_custom_origin(Some((x, y)));

    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    match serde_json::to_string(&(x, y)) {
        Ok(raw) => {
            if let Err(e) = state.db.set_setting(PILL_POSITION_KEY, &raw) {
                warn!("Failed to persist pill position: {e}");
            }
        }
        Err(e) => warn!("Failed to serialize pill position: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
        let text = "caf\u{e9} au lait \u{2615}"; // "café au lait ☕"
        let tail = live_text_tail(text, 5);
        assert_eq!(tail.chars().count(), 5);
        assert!(String::from_utf8(tail.into_bytes()).is_ok());
    }

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
            "releasing with nothing held is a safe no-op"
        );
        assert!(!is_held());
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

    #[test]
    fn mode_copy_matches_frontend_i18n() {
        assert_eq!(mode_title(PillPanelMode::Dictation, false), "Dictating");
        assert_eq!(mode_title(PillPanelMode::Dictation, true), "Dictée");
        assert_eq!(
            mode_title(PillPanelMode::Polishing, false),
            "Reformulating…"
        );
        assert_eq!(mode_title(PillPanelMode::Polishing, true), "Reformulation…");
        assert_eq!(mode_title(PillPanelMode::Meeting, false), "");
        assert_eq!(
            a11y_label(PillPanelMode::Dictation, true),
            "Dictée en cours"
        );
        assert_eq!(stop_label(false), "Stop recording");
        assert_eq!(stop_label(true), "Arrêter l'enregistrement");
    }
}
