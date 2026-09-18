//! Cross-crate action dispatch (SOU-191).
//!
//! `souffle_lib` has no knowledge of the Slint `MainWindow` (it lives in the
//! `souffle-slint` crate), but native OS surfaces it *does* own — the tray
//! menu, the floating pill's HUD stop button, the global keyboard shortcuts —
//! need to reach into it (start/stop a session, show the window, switch
//! view). Before this ticket that gap was papered over with
//! `tauri_specta::Event::emit`, which only ever had a listener when the old
//! Svelte webview was still around; grepped, there is now no `.listen(` call
//! anywhere in the tree, so every one of those emits was already inert.
//!
//! This is the replacement: `souffle-slint::main()` calls [`set_sink`] once
//! at startup with a channel it owns, and native code calls [`dispatch`].
//! Nothing here is Tauri-specific; a plain `crossbeam_channel` is enough
//! since both ends are the same process.

use std::sync::Mutex;

use crossbeam_channel::Sender;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppView {
    Home,
    Settings,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NativeAction {
    /// The canonical state machine changed. The UI re-reads its snapshot;
    /// no second state or potentially stale event payload is maintained.
    RefreshRuntime,
    /// Global shortcut / tray menu: start dictation if idle, stop if
    /// recording a dictation. A no-op while a meeting is recording (SOU-044:
    /// a meeting owns the session).
    ToggleDictation,
    /// Native push-to-talk key pressed.
    PttStart,
    /// Native push-to-talk key released.
    PttStop,
    /// Pill HUD stop button, or tray "Stop Meeting Recording".
    StopMeeting,
    /// Pill HUD stop button, routed to dictation instead of a meeting.
    StopDictation,
    /// Escape pressed during a cancelable toggle dictation (SOU-117).
    CancelDictation,
    Navigate(AppView),
    ShowMainWindow,
    /// A newer GitHub release was found by the background scheduler.
    UpdateAvailable {
        latest_version: String,
        release_notes: Option<String>,
        release_url: Option<String>,
    },
}

static ACTION_SINK: Mutex<Option<Sender<NativeAction>>> = Mutex::new(None);

/// Called once by `souffle-slint::main()` before any native module (tray,
/// shortcuts, pill) can produce an action that needs the Slint window.
pub fn set_sink(tx: Sender<NativeAction>) {
    if let Ok(mut guard) = ACTION_SINK.lock() {
        *guard = Some(tx);
    }
}

/// Send an action to whoever is listening (`souffle-slint`'s receiver
/// thread). Silently dropped if no sink is registered yet (startup race) or
/// the receiver is gone (shutdown race) — neither is a correctness issue,
/// only a missed UI update for an action taken at a bad moment.
pub fn dispatch(action: NativeAction) {
    if let Ok(guard) = ACTION_SINK.lock()
        && let Some(tx) = guard.as_ref()
    {
        let _ = tx.send(action);
    }
}
