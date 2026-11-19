//! Onboarding wizard (SOU-190). Row/step formatting only - the actual TCC
//! polling and model/shortcut orchestration live in `main.rs`'s
//! `wire_onboarding_callbacks`, reusing the same commands
//! (`get_permission_status`, `get_model_status`/`download_model`/
//! `load_model`, `save_shortcuts`) the Settings tab and the recording flow
//! already call.
//!
//! Simplified versus `PermissionsStep.svelte`: the "stale TCC entry"
//! diagnosis (tracked via a blur/focus attempt-count pair) is not
//! reproduced - Accessibility always shows the plain denied hint, and
//! "Repair" is always available rather than gated behind two failed
//! attempts. The repair action itself is real; only that one UX nicety
//! is dropped.

use crate::{MainWindow, PermissionRow, settings_ui};
use slint::Model;
use souffle_lib::permissions::{PermState, PermissionKind, PermissionStatus};

const ROWS: [PermissionKind; 3] = [
    PermissionKind::Microphone,
    PermissionKind::SystemAudio,
    PermissionKind::Accessibility,
];

fn state_of(status: &PermissionStatus, kind: PermissionKind) -> PermState {
    match kind {
        PermissionKind::Microphone => status.microphone,
        PermissionKind::SystemAudio => status.system_audio,
        PermissionKind::Accessibility => status.accessibility,
        PermissionKind::Calendar => status.calendar,
    }
}

/// Rows are rewritten in place when their count is unchanged, so a press in
/// progress on a row's button survives the 600 ms permission poll.
pub fn populate_permission_rows(window: &MainWindow, status: &PermissionStatus, busy: [bool; 3]) {
    let rows: Vec<PermissionRow> = ROWS
        .iter()
        .enumerate()
        .map(|(i, &kind)| {
            let state = state_of(status, kind);
            PermissionRow {
                kind: settings_ui::permission_kind_to_slint(kind),
                state: settings_ui::perm_state_to_slint(state),
                busy: busy[i],
            }
        })
        .collect();
    let current = window.get_onboarding_permission_rows();
    if let Some(model) = current
        .as_any()
        .downcast_ref::<slint::VecModel<PermissionRow>>()
        && model.row_count() == rows.len()
    {
        for (index, row) in rows.into_iter().enumerate() {
            if model.row_data(index).as_ref() != Some(&row) {
                model.set_row_data(index, row);
            }
        }
        return;
    }
    window.set_onboarding_permission_rows(std::rc::Rc::new(slint::VecModel::from(rows)).into());
}

pub fn row_index(kind: PermissionKind) -> Option<usize> {
    ROWS.iter().position(|&k| k == kind)
}
