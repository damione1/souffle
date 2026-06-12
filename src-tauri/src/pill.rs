//! The floating recording pill — a small always-on-top window shown while
//! any recording (dictation or meeting) is active, so the user gets visual
//! feedback even when the main window is hidden. Visibility is driven from
//! the backend's state transitions (the single source of truth); content
//! and the stop action live in the pill's webview (`src/lib/pill/`).

use tauri::{AppHandle, LogicalPosition, Manager};
use tracing::warn;

use crate::state_machine::AppStateMachine;

/// Vertical offset below the menu bar.
const TOP_MARGIN: f64 = 40.0;

/// Show the pill while recording, hide it otherwise. Called on every state
/// transition; must never steal focus from the app the user is dictating
/// into (the window is configured with `focus: false` and we only ever
/// call `show`, never `set_focus`).
pub fn sync(app: &AppHandle, machine: &AppStateMachine) {
    let Some(pill) = app.get_webview_window("pill") else {
        return;
    };

    let recording = matches!(
        machine,
        AppStateMachine::RecordingDictation { .. } | AppStateMachine::RecordingMeeting { .. }
    );

    let result = if recording {
        position_top_center(&pill).and_then(|()| pill.show())
    } else {
        pill.hide()
    };
    if let Err(e) = result {
        warn!("Recording pill sync failed: {e}");
    }
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
