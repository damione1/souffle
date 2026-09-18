use std::sync::Arc;

use crate::app_events::PillHoldKind;
use crate::state::AppState;

/// Ask the floating pill to stay visible even though the state machine left
/// a recording state — used while dictation polish reformulates in the
/// background after transcription stops. Called *before* stop, while still
/// recording; `pill::sync` must not drop a hold just because the machine is
/// currently in a recording state (it only clears leftover holds when a
/// *new* session starts).
pub fn pill_hold(state: Arc<AppState>, kind: PillHoldKind) -> Result<(), String> {
    crate::pill::set_hold(kind);
    crate::pill::sync(&state, &state.current_machine_state()?);
    Ok(())
}

/// Release a hold set by `pill_hold`. Safe to call with nothing held (e.g.
/// paste succeeded without dictation polish ever engaging a hold).
pub fn pill_release(state: Arc<AppState>) -> Result<(), String> {
    crate::pill::clear_hold();
    crate::pill::sync(&state, &state.current_machine_state()?);
    Ok(())
}
