//! The meeting writer owns its id until every DB/audio writer has gone away.
//! Reservations and deletion use the same mutex. It is never held across an
//! await, actor reply, or native/window dispatch. Lock order: access -> machine
//! -> lease phase; DB is acquired only after the permission has been checked.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use crate::lock_ext::MutexExt;
use crate::state_machine::{AppStateMachine, RecordingKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingDeleteBlockReason {
    Starting,
    Recording,
    Finalizing,
}

#[derive(Debug, thiserror::Error)]
pub enum MeetingDeleteError {
    #[error("{}", .0.message())]
    Blocked(MeetingDeleteBlockReason),
    #[error("{0}")]
    Failed(String),
}

impl MeetingDeleteBlockReason {
    pub fn message(self) -> &'static str {
        match self {
            Self::Starting => "This meeting is starting",
            Self::Recording => "This meeting is recording",
            Self::Finalizing => "This meeting is finalizing",
        }
    }
}

#[derive(Debug)]
struct LeaseInner {
    phase: Arc<Mutex<MeetingDeleteBlockReason>>,
}

impl Drop for LeaseInner {
    fn drop(&mut self) {
        permissions_changed();
    }
}

/// Clone only when handing ownership to an actual writer. The registry holds
/// weak references, so the last writer's drop releases the reservation even
/// when a start/stop future is cancelled or a writer unwinds.
#[derive(Debug, Clone)]
pub struct MeetingLease(Arc<LeaseInner>);

impl MeetingLease {
    pub(crate) fn set_phase(&self, phase: MeetingDeleteBlockReason) {
        {
            let mut current = self.0.phase.lock().unwrap_or_else(|p| p.into_inner());
            if *current == phase {
                return;
            }
            *current = phase;
        }
        permissions_changed();
    }
}

struct MeetingWriter {
    owner: Weak<LeaseInner>,
    // Read the phase without upgrading ownership: a permission read must
    // never become the final owner and emit its drop event under our locks.
    phase: Arc<Mutex<MeetingDeleteBlockReason>>,
}

#[derive(Default)]
pub(crate) struct MeetingAccess {
    writers: Mutex<HashMap<String, MeetingWriter>>,
}

impl MeetingAccess {
    pub(crate) fn reserve(&self, id: &str) -> Result<MeetingLease, String> {
        let mut writers = self.writers.acquire()?;
        writers.retain(|_, writer| writer.owner.strong_count() != 0);
        if writers.contains_key(id) {
            return Err("This meeting already has an active writer".into());
        }
        let phase = Arc::new(Mutex::new(MeetingDeleteBlockReason::Starting));
        let lease = MeetingLease(Arc::new(LeaseInner {
            phase: Arc::clone(&phase),
        }));
        writers.insert(
            id.to_string(),
            MeetingWriter {
                owner: Arc::downgrade(&lease.0),
                phase,
            },
        );
        drop(writers);
        permissions_changed();
        Ok(lease)
    }

    pub(crate) fn block(
        &self,
        machine: &Mutex<AppStateMachine>,
        id: &str,
    ) -> Result<Option<MeetingDeleteBlockReason>, String> {
        let writers = self.writers.acquire()?;
        let machine = machine.acquire()?;
        blocked_reason(&writers, &machine, id)
    }

    pub(crate) fn delete<T>(
        &self,
        machine: &Mutex<AppStateMachine>,
        id: &str,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, MeetingDeleteError> {
        let writers = self.writers.acquire().map_err(MeetingDeleteError::Failed)?;
        // Keep both locks until DB and disk deletion have finished. Normal
        // start/resume are protected by their lease before touching either.
        let machine = machine.acquire().map_err(MeetingDeleteError::Failed)?;
        let block = blocked_reason(&writers, &machine, id).map_err(MeetingDeleteError::Failed)?;
        guard_action(block, action)
            .map_err(MeetingDeleteError::Blocked)?
            .map_err(MeetingDeleteError::Failed)
    }
}

/// Shared early-exit helper for UI handlers and the serialized backend action.
/// No side effect belongs before this call; the reason is also what UI projects.
pub fn guard_action<T>(
    block: Option<MeetingDeleteBlockReason>,
    action: impl FnOnce() -> T,
) -> Result<T, MeetingDeleteBlockReason> {
    match block {
        Some(reason) => Err(reason),
        None => Ok(action()),
    }
}

fn blocked_reason(
    writers: &HashMap<String, MeetingWriter>,
    machine: &AppStateMachine,
    id: &str,
) -> Result<Option<MeetingDeleteBlockReason>, String> {
    let state_reason = match machine {
        AppStateMachine::RecordingMeeting { meeting_id, .. } if meeting_id == id => {
            Some(MeetingDeleteBlockReason::Recording)
        }
        AppStateMachine::Stopping { was_recording, .. } => match was_recording {
            RecordingKind::Meeting { meeting_id } if meeting_id == id => {
                Some(MeetingDeleteBlockReason::Finalizing)
            }
            RecordingKind::Meeting { .. } | RecordingKind::Dictation => None,
        },
        AppStateMachine::Idle
        | AppStateMachine::Downloading { .. }
        | AppStateMachine::Downloaded { .. }
        | AppStateMachine::Loading { .. }
        | AppStateMachine::Ready { .. }
        | AppStateMachine::RecordingDictation { .. }
        | AppStateMachine::RecordingMeeting { .. }
        | AppStateMachine::Unloading { .. }
        | AppStateMachine::Error { .. } => None,
    };
    if state_reason.is_some() {
        return Ok(state_reason);
    }
    writers
        .get(id)
        .filter(|writer| writer.owner.strong_count() != 0)
        .map(|writer| writer.phase.acquire().map(|phase| *phase))
        .transpose()
}

fn permissions_changed() {
    crate::native::bridge::dispatch(
        crate::native::bridge::NativeAction::MeetingDeletePermissionsChanged,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::default_transcription_profile;
    use crate::state_machine::ErrorRecovery;

    #[test]
    fn guard_exits_before_the_action_for_every_block_reason() {
        for reason in [
            MeetingDeleteBlockReason::Starting,
            MeetingDeleteBlockReason::Recording,
            MeetingDeleteBlockReason::Finalizing,
        ] {
            let effects = std::cell::Cell::new(0);
            assert_eq!(
                guard_action(Some(reason), || effects.set(effects.get() + 1)),
                Err(reason)
            );
            assert_eq!(effects.get(), 0);
            guard_action(None, || effects.set(effects.get() + 1)).unwrap();
            assert_eq!(effects.get(), 1);
        }
    }

    #[test]
    fn every_state_without_meeting_ownership_allows_delete() {
        let p = default_transcription_profile();
        for machine in [
            AppStateMachine::Idle,
            AppStateMachine::Downloading { profile: p.clone() },
            AppStateMachine::Downloaded { profile: p.clone() },
            AppStateMachine::Loading { profile: p.clone() },
            AppStateMachine::Ready { profile: p.clone() },
            AppStateMachine::RecordingDictation {
                profile: p.clone(),
                session_id: 1,
            },
            AppStateMachine::Stopping {
                profile: p.clone(),
                was_recording: RecordingKind::Dictation,
            },
            AppStateMachine::Unloading {
                profile: p.clone(),
                next_profile: None,
            },
            AppStateMachine::Error {
                source: crate::state_machine::FailureSource::AudioCapture,
                message: "test".into(),
                recovery: ErrorRecovery::RetryFromIdle,
            },
        ] {
            let access = MeetingAccess::default();
            let machine = Mutex::new(machine);
            assert_eq!(access.block(&machine, "old").unwrap(), None);
        }
    }

    #[test]
    fn reservation_and_delete_are_serialized_until_the_disk_action_finishes() {
        let access = Arc::new(MeetingAccess::default());
        let machine = Arc::new(Mutex::new(AppStateMachine::Idle));
        let (entered_tx, entered_rx) = crossbeam_channel::bounded(1);
        let (release_tx, release_rx) = crossbeam_channel::bounded(1);
        let target = Arc::clone(&access);
        let state = Arc::clone(&machine);
        let delete = std::thread::spawn(move || {
            target.delete(&state, "meeting", || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(matches!(
            access.writers.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        let (attempt_tx, attempt_rx) = crossbeam_channel::bounded(1);
        let target = Arc::clone(&access);
        let start = std::thread::spawn(move || {
            attempt_tx.send(()).unwrap();
            target.reserve("meeting")
        });
        attempt_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        release_tx.send(()).unwrap();
        delete.join().unwrap().unwrap();
        let lease = start.join().unwrap().unwrap();
        assert_eq!(
            access.block(&machine, "meeting").unwrap(),
            Some(MeetingDeleteBlockReason::Starting)
        );
        assert!(access.delete(&machine, "meeting", || Ok(())).is_err());
        drop(lease);
        access.delete(&machine, "meeting", || Ok(())).unwrap();
    }
}
