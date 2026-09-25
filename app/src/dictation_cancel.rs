//! Transient Escape binding that discards an in-progress toggle dictation.
//!
//! Armed only while the machine is `RecordingDictation` *and* the session
//! asked for it (toggle, not push-to-talk). `native::shortcuts::register_shortcuts`
//! calls `unregister_all`, so this module re-applies the binding afterwards
//! when a cancelable session is still live. The actual keypress -> action
//! dispatch lives in `native::shortcuts::spawn_event_loop` (SOU-191); this
//! module only decides whether the binding should currently exist.

use std::sync::atomic::{AtomicBool, Ordering};

use tracing::warn;

use crate::native::shortcuts;
use crate::settings::ShortcutSettings;
use crate::state::AppState;
use crate::state_machine::AppStateMachine;

static WANTED: AtomicBool = AtomicBool::new(false);
static ARMED: AtomicBool = AtomicBool::new(false);

/// Whether Escape should currently steal the key from the foreground app.
pub fn should_arm(wanted: bool, machine: &AppStateMachine) -> bool {
    wanted && matches!(machine, AppStateMachine::RecordingDictation { .. })
}

pub fn set_wanted(wanted: bool) {
    WANTED.store(wanted, Ordering::SeqCst);
}

/// `register_shortcuts` just wiped every binding, including ours.
pub fn mark_unregistered() {
    ARMED.store(false, Ordering::SeqCst);
}

pub fn sync(state: &AppState, machine: &AppStateMachine) {
    let in_dictation = matches!(machine, AppStateMachine::RecordingDictation { .. });
    if !in_dictation {
        WANTED.store(false, Ordering::SeqCst);
    }
    let should = should_arm(WANTED.load(Ordering::SeqCst), machine);
    let armed = ARMED.load(Ordering::SeqCst);
    if should == armed {
        return;
    }

    if should {
        arm();
    } else {
        disarm(state);
    }
}

fn arm() {
    match shortcuts::arm_escape_cancel() {
        Ok(()) => {
            ARMED.store(true, Ordering::SeqCst);
            tracing::info!("Dictation cancel shortcut armed");
        }
        Err(e) => {
            warn!(error = %e, "Failed to arm Escape dictation cancel");
        }
    }
}

fn disarm(state: &AppState) {
    if let Err(e) = shortcuts::disarm_escape() {
        warn!(error = %e, "Failed to unregister Escape dictation cancel");
        return;
    }
    ARMED.store(false, Ordering::SeqCst);
    restore_user_escape(state);
    tracing::info!("Dictation cancel shortcut disarmed");
}

fn restore_user_escape(state: &AppState) {
    let Ok(shortcuts_settings) = ShortcutSettings::load(&state.db) else {
        return;
    };
    if let Err(e) = shortcuts::restore_escape_role(&shortcuts_settings) {
        warn!(error = %e, "Failed to restore Escape as a user shortcut");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::default_transcription_profile;
    use crate::state_machine::RecordingKind;

    fn profile() -> crate::engine::TranscriptionProfile {
        default_transcription_profile()
    }

    #[test]
    fn arms_only_while_a_wanted_dictation_is_recording() {
        let dictation = AppStateMachine::RecordingDictation {
            profile: profile(),
            session_id: 1,
        };
        let meeting = AppStateMachine::RecordingMeeting {
            profile: profile(),
            session_id: 1,
            meeting_id: "m1".into(),
        };
        let stopping = AppStateMachine::Stopping {
            profile: profile(),
            was_recording: RecordingKind::Dictation,
        };
        let ready = AppStateMachine::Ready { profile: profile() };
        let error = AppStateMachine::Error {
            message: "boom".into(),
            recovery: crate::state_machine::ErrorRecovery::RetryFromReady { profile: profile() },
        };

        assert!(should_arm(true, &dictation));
        assert!(!should_arm(false, &dictation));
        assert!(!should_arm(true, &meeting));
        assert!(!should_arm(true, &stopping));
        assert!(!should_arm(true, &ready));
        assert!(!should_arm(true, &error));
        assert!(!should_arm(true, &AppStateMachine::Idle));
    }
}
