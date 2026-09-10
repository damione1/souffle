//! Transient Escape binding that discards an in-progress toggle dictation.
//!
//! Armed only while the machine is `RecordingDictation` *and* the session
//! asked for it (toggle, not push-to-talk). `register_shortcuts` calls
//! `unregister_all`, so this module re-applies the binding afterwards when
//! a cancelable session is still live.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_specta::Event;
use tracing::warn;

use crate::app_events::{
    DictationCancelRequested, ShortcutPttStart, ShortcutPttStop, ShortcutToggle,
};
use crate::modifier_shortcut::is_native_ptt_shortcut;
use crate::settings::ShortcutSettings;
use crate::state::AppState;
use crate::state_machine::AppStateMachine;

const ESCAPE: &str = "Escape";

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

pub fn sync(app: &AppHandle, machine: &AppStateMachine) {
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
        arm(app);
    } else {
        disarm(app);
    }
}

fn arm(app: &AppHandle) {
    let gs = app.global_shortcut();
    if gs.is_registered(ESCAPE)
        && let Err(e) = gs.unregister(ESCAPE)
    {
        warn!(error = %e, "Failed to replace Escape binding for dictation cancel");
        return;
    }
    match gs.on_shortcut(ESCAPE, |app, _shortcut, event| {
        if event.state == ShortcutState::Pressed {
            let _ = DictationCancelRequested.emit(app);
        }
    }) {
        Ok(()) => {
            ARMED.store(true, Ordering::SeqCst);
            tracing::info!("Dictation cancel shortcut armed");
        }
        Err(e) => {
            warn!(error = %e, "Failed to arm Escape dictation cancel");
            restore_user_escape(app);
        }
    }
}

fn disarm(app: &AppHandle) {
    let gs = app.global_shortcut();
    if gs.is_registered(ESCAPE)
        && let Err(e) = gs.unregister(ESCAPE)
    {
        warn!(error = %e, "Failed to unregister Escape dictation cancel");
        return;
    }
    ARMED.store(false, Ordering::SeqCst);
    restore_user_escape(app);
    tracing::info!("Dictation cancel shortcut disarmed");
}

fn restore_user_escape(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let Ok(shortcuts) = ShortcutSettings::load(&state.db) else {
        return;
    };
    let gs = app.global_shortcut();

    if shortcuts.toggle == ESCAPE {
        if let Err(e) = gs.on_shortcut(ESCAPE, |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                let _ = ShortcutToggle.emit(app);
            }
        }) {
            warn!(error = %e, "Failed to restore Escape as toggle shortcut");
        }
        return;
    }

    if shortcuts.push_to_talk != ESCAPE || is_native_ptt_shortcut(&shortcuts.push_to_talk) {
        return;
    }
    if let Err(e) = gs.on_shortcut(ESCAPE, |app, _shortcut, event| match event.state {
        ShortcutState::Pressed => {
            let state = app.state::<AppState>();
            if state.ptt_is_paused() {
                state.ptt_start_armed.store(false, Ordering::SeqCst);
            } else {
                state.ptt_start_armed.store(true, Ordering::SeqCst);
                let _ = ShortcutPttStart.emit(app);
            }
        }
        ShortcutState::Released => {
            if app
                .state::<AppState>()
                .ptt_start_armed
                .swap(false, Ordering::SeqCst)
            {
                let _ = ShortcutPttStop.emit(app);
            }
        }
    }) {
        warn!(error = %e, "Failed to restore Escape as push-to-talk shortcut");
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
