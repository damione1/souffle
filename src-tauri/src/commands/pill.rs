use tauri::{AppHandle, Manager};

use crate::app_events::PillHoldKind;
use crate::state::AppState;

/// Ask the floating pill to stay visible even though the state machine left
/// a recording state — used while dictation polish reformulates in the
/// background after transcription stops. Called *before* stop, while still
/// recording; `pill::sync` must not drop a hold just because the machine is
/// currently in a recording state (it only clears leftover holds when a
/// *new* session starts).
///
/// Takes `AppHandle` directly rather than `State<'_, AppState>`: this command
/// runs on the main thread, and reading `AppState.app_handle` (a mutex) would
/// risk deadlocking against `AppState::apply_transition`, which briefly holds
/// that same mutex from a background thread while dispatching to the main
/// thread for window operations.
#[tauri::command]
#[specta::specta]
pub fn pill_hold(app: AppHandle, kind: PillHoldKind) -> Result<(), String> {
    crate::pill::set_hold(&app, kind);
    let state = app.state::<AppState>();
    crate::pill::sync(&app, &state.current_machine_state()?);
    Ok(())
}

/// Release a hold set by `pill_hold`. Safe to call with nothing held (e.g.
/// paste succeeded without dictation polish ever engaging a hold).
#[tauri::command]
#[specta::specta]
pub fn pill_release(app: AppHandle) -> Result<(), String> {
    crate::pill::clear_hold(&app);
    let state = app.state::<AppState>();
    crate::pill::sync(&app, &state.current_machine_state()?);
    Ok(())
}

/// Resize the pill window to `width` x `height` (logical pixels). A
/// dragged position is kept (and clamped on-screen); otherwise the pill
/// stays top-centered. The frontend calls this as the live transcript
/// grows/shrinks. A single native frame change avoids the top-edge drift
/// that a separate resize-then-recenter pair produces.
#[tauri::command]
#[specta::specta]
pub fn pill_resize(app: AppHandle, width: f64, height: f64) -> Result<(), String> {
    let Some(pill) = app.get_webview_window("pill") else {
        return Ok(());
    };
    crate::pill::set_frame_top_center(&pill, width, height).map_err(|e| e.to_string())
}
