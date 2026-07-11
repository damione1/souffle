use tauri::{Manager, State};

use crate::app_events::PillHoldKind;
use crate::state::AppState;

/// Ask the floating pill to stay visible even though the state machine left
/// a recording state — used while dictation polish reformulates in the
/// background after transcription stops. See `pill::sync` for how a hold
/// combines with state-machine-driven visibility, and its safety net (a hold
/// is auto-released the moment a new recording starts).
#[tauri::command]
#[specta::specta]
pub fn pill_hold(state: State<'_, AppState>, kind: PillHoldKind) -> Result<(), String> {
    let app = state.app_handle()?;
    crate::pill::set_hold(&app, kind);
    crate::pill::sync(&app, &state.current_machine_state()?);
    Ok(())
}

/// Release a hold set by `pill_hold`. Safe to call with nothing held (e.g.
/// paste succeeded without dictation polish ever engaging a hold).
#[tauri::command]
#[specta::specta]
pub fn pill_release(state: State<'_, AppState>) -> Result<(), String> {
    let app = state.app_handle()?;
    crate::pill::clear_hold(&app);
    crate::pill::sync(&app, &state.current_machine_state()?);
    Ok(())
}

/// Resize the pill window to `width` x `height` (logical pixels), keeping
/// its top edge pinned below the menu bar and staying horizontally
/// centered. The frontend calls this as the live transcript grows/shrinks
/// (e.g. switching between the compact and expanded live-text layouts): a
/// single native frame change avoids the top-edge drift that a separate
/// resize-then-recenter pair produces.
#[tauri::command]
#[specta::specta]
pub fn pill_resize(state: State<'_, AppState>, width: f64, height: f64) -> Result<(), String> {
    let app = state.app_handle()?;
    let Some(pill) = app.get_webview_window("pill") else {
        return Ok(());
    };
    crate::pill::set_frame_top_center(&pill, width, height).map_err(|e| e.to_string())
}
