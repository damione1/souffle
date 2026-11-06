use crate::{ActionGuards, MainWindow, MeetingDeleteBlockReason, MeetingDeletePermission};
use slint::ComponentHandle;
use souffle_lib::meeting_access::{
    MeetingDeleteBlockReason as Reason, MeetingDeleteError, guard_action,
};
use souffle_lib::state::AppState;
use std::sync::Arc;

pub fn reason_to_slint(reason: Reason) -> MeetingDeleteBlockReason {
    match reason {
        Reason::Starting => MeetingDeleteBlockReason::Starting,
        Reason::Recording => MeetingDeleteBlockReason::Recording,
        Reason::Finalizing => MeetingDeleteBlockReason::Finalizing,
    }
}

#[cfg(test)]
pub fn reason_from_slint(reason: MeetingDeleteBlockReason) -> Reason {
    match reason {
        MeetingDeleteBlockReason::Starting => Reason::Starting,
        MeetingDeleteBlockReason::Recording => Reason::Recording,
        MeetingDeleteBlockReason::Finalizing => Reason::Finalizing,
    }
}

pub fn permission(block: Option<Reason>) -> MeetingDeletePermission {
    MeetingDeletePermission {
        blocked: block.is_some(),
        reason: reason_to_slint(block.unwrap_or(Reason::Starting)),
        check_failed: false,
    }
}

pub fn install(window: &MainWindow, state: &Arc<AppState>) {
    let handle = Arc::clone(state);
    window
        .global::<ActionGuards>()
        .on_meeting_delete(move |id, _revision| {
            match souffle_lib::commands::get_meeting_delete_block(&handle, &id) {
                Ok(block) => permission(block),
                Err(error) => {
                    eprintln!("Cannot check meeting deletion: {error}");
                    MeetingDeletePermission {
                        blocked: true,
                        check_failed: true,
                        ..permission(None)
                    }
                }
            }
        });
    refresh(window);
}

pub fn refresh(window: &MainWindow) {
    let guards = window.global::<ActionGuards>();
    guards.set_revision(guards.get_revision().wrapping_add(1));
}

/// Refused handlers return before even invoking the backend command. A state
/// change after this read is caught again by the backend's serialized guard.
pub fn delete_from_handler(state: &Arc<AppState>, id: &str) -> Result<(), MeetingDeleteError> {
    let block = souffle_lib::commands::get_meeting_delete_block(state, id)
        .map_err(MeetingDeleteError::Failed)?;
    guard_action(block, || {
        souffle_lib::commands::delete_meeting_guarded(Arc::clone(state), id.to_string())
    })
    .map_err(MeetingDeleteError::Blocked)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_reason_roundtrips_every_variant() {
        for reason in [Reason::Starting, Reason::Recording, Reason::Finalizing] {
            let projected = permission(Some(reason));
            assert!(projected.blocked);
            assert_eq!(reason_from_slint(projected.reason), reason);
        }
        assert!(!permission(None).blocked);
    }
}
