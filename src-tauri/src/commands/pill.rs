use tauri::State;

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
