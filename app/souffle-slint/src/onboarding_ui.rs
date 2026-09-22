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
    window.set_onboarding_permission_rows(std::rc::Rc::new(slint::VecModel::from(rows)).into());
}

pub fn row_index(kind: PermissionKind) -> Option<usize> {
    ROWS.iter().position(|&k| k == kind)
}
