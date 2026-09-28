//! Resume after sleep (SOU-295): the meeting the Mac's sleep stopped is
//! resumed once it wakes up.
//!
//! On will-sleep the backend stops the meeting like a user stop and keeps its
//! id (`commands::peek_sleep_paused_meeting`); on did-wake it sends
//! `NativeAction::SystemWokeUp`. The Tauri meeting controller
//! (`src/lib/features/meeting/controller.svelte.ts`, `resumeAfterSystemWake`)
//! took it from there, and this does the same:
//!
//! - the sleep-triggered stop runs off the will-sleep callback, so it is often
//!   still draining at wake: wait for the machine to settle (every state
//!   change re-checks), for at most [`WAKE_RESUME_TIMEOUT`];
//! - then open the meeting and resume it through the detail's own Resume
//!   handler, and say "Recording resumed after sleep";
//! - if the wait times out, open the meeting with "Sleep interrupted this
//!   meeting" so its Resume button is at hand, and leaving it without resuming
//!   drops the offer (`commands::clear_sleep_paused_meeting`).
//!
//! The detail's one-hour resume window does not apply to that meeting while
//! the backend still holds its id (see `meeting_can_resume`).

use std::cell::RefCell;
use std::time::Duration;

use slint::ComponentHandle;
use souffle_lib::state_machine::AppStateMachine;

use crate::{AppHandle, MainWindow, RecordingMode, TimelineKind, WakeResumeNotice};

/// How long a wake-resume waits for the sleep-triggered stop to finish
/// before falling back to the manual Resume button (the Tauri controller's
/// `WAKE_RESUME_TIMEOUT_MS`).
pub const WAKE_RESUME_TIMEOUT: Duration = Duration::from_secs(8);

/// How long the notice stays up (the Tauri controller's `AUTO_HIDE_MS` for a
/// banner without an action).
const NOTICE_AUTO_HIDE: Duration = Duration::from_secs(5);

/// What a wake, or a state change while a wake-resume waits, calls for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeResumeStep {
    /// No meeting was stopped by sleep, or it was already resumed.
    Nothing,
    /// The machine or the UI is still busy: check again on the next state
    /// change, until the timeout.
    Wait(String),
    Resume(String),
}

/// `sleep_paused` is `peek_sleep_paused_meeting`; `ui_starting` is the
/// recording view's "Starting…" (a session the user just asked for, which
/// clears the backend id itself once it launches).
pub fn wake_resume_step(
    sleep_paused: Option<String>,
    machine: &AppStateMachine,
    ui_starting: bool,
) -> WakeResumeStep {
    let Some(meeting_id) = sleep_paused else {
        return WakeResumeStep::Nothing;
    };
    if ui_starting {
        return WakeResumeStep::Wait(meeting_id);
    }
    match machine {
        // The sleep-triggered stop has not transitioned yet, or is draining;
        // or the model is changing hands. Each ends in a state change.
        AppStateMachine::RecordingMeeting { .. }
        | AppStateMachine::RecordingDictation { .. }
        | AppStateMachine::Stopping { .. }
        | AppStateMachine::Loading { .. }
        | AppStateMachine::Unloading { .. }
        | AppStateMachine::Downloading { .. } => WakeResumeStep::Wait(meeting_id),
        // The resume path loads the model if it has to, and reports its own
        // failure in the detail, next to the Resume button.
        AppStateMachine::Ready { .. }
        | AppStateMachine::Idle
        | AppStateMachine::Downloaded { .. }
        | AppStateMachine::Error { .. } => WakeResumeStep::Resume(meeting_id),
    }
}

#[derive(Default)]
struct WakeResume {
    /// The meeting a wake is waiting to resume.
    pending: Option<String>,
    timeout: slint::Timer,
    /// The meeting being resumed by a wake, until the resume settles.
    resuming: Option<String>,
    /// The meeting opened by the fallback: leaving it without resuming
    /// drops the backend's offer.
    fallback: Option<String>,
    notice_timer: slint::Timer,
}

thread_local! {
    // UI thread only: every entry point runs on the Slint event loop.
    static WAKE_RESUME: RefCell<WakeResume> = RefCell::new(WakeResume::default());
}

fn current_step(window: &MainWindow, handle: &AppHandle) -> WakeResumeStep {
    let Ok(machine) = handle.current_machine_state() else {
        return WakeResumeStep::Nothing;
    };
    wake_resume_step(
        handle.peek_sleep_paused_meeting(),
        &machine,
        window.get_recording_starting(),
    )
}

/// `NativeAction::SystemWokeUp`.
pub fn system_woke(window: &MainWindow, handle: &AppHandle) {
    match current_step(window, handle) {
        WakeResumeStep::Nothing => {}
        WakeResumeStep::Wait(meeting_id) => arm(window, handle, meeting_id),
        WakeResumeStep::Resume(meeting_id) => {
            disarm();
            resume(window, handle, &meeting_id);
        }
    }
}

/// `NativeAction::RefreshRuntime`: a no-op unless a wake-resume waits.
pub fn state_changed(window: &MainWindow, handle: &AppHandle) {
    if WAKE_RESUME.with(|slot| slot.borrow().pending.is_none()) {
        return;
    }
    match current_step(window, handle) {
        WakeResumeStep::Wait(_) => {}
        // Resumed by hand, or by a new meeting, meanwhile.
        WakeResumeStep::Nothing => disarm(),
        WakeResumeStep::Resume(meeting_id) => {
            disarm();
            resume(window, handle, &meeting_id);
        }
    }
}

/// Re-arming (a second wake while already waiting) restarts the wait.
fn arm(window: &MainWindow, handle: &AppHandle, meeting_id: String) {
    let weak = window.as_weak();
    let handle = std::sync::Arc::clone(handle);
    WAKE_RESUME.with(|slot| {
        let mut state = slot.borrow_mut();
        state.pending = Some(meeting_id.clone());
        state.timeout.start(
            slint::TimerMode::SingleShot,
            WAKE_RESUME_TIMEOUT,
            move || {
                let still_pending = WAKE_RESUME.with(|slot| {
                    let mut state = slot.borrow_mut();
                    let still_pending = state.pending.as_deref() == Some(meeting_id.as_str());
                    if still_pending {
                        state.pending = None;
                    }
                    still_pending
                });
                if still_pending && let Some(window) = weak.upgrade() {
                    fall_back_to_manual_resume(&window, &handle, &meeting_id);
                }
            },
        );
    });
}

fn disarm() {
    WAKE_RESUME.with(|slot| {
        let mut state = slot.borrow_mut();
        state.pending = None;
        state.timeout.stop();
    });
}

/// The recording view still shows the meeting the sleep stopped behind the
/// UI's back: close it as a stop would, keeping the notes typed since the
/// last autosave.
fn close_stale_meeting_view(window: &MainWindow, handle: &AppHandle, meeting_id: &str) {
    if window.get_recording_mode() != RecordingMode::Meeting || window.get_recording_starting() {
        return;
    }
    // A sleep-triggered stop that failed left the meeting recording: the
    // view is not stale then.
    if matches!(
        handle.current_machine_state(),
        Ok(AppStateMachine::RecordingMeeting { .. })
    ) {
        return;
    }
    // Through the command, not the DB: a stop still draining keeps the
    // notes of its accumulator, and would write the older ones back.
    let notes = window.get_live_notes().to_string();
    if !notes.trim().is_empty()
        && let Err(e) = souffle_lib::commands::save_meeting_notes(
            std::sync::Arc::clone(handle),
            meeting_id.to_string(),
            Some(notes),
        )
    {
        eprintln!("Failed to save the notes of a meeting stopped by sleep: {e}");
    }
    window.set_live_notes("".into());
    crate::set_recording_idle(window);
}

fn open_meeting(window: &MainWindow, meeting_id: &str) -> bool {
    window.invoke_timeline_item_opened(TimelineKind::Meeting, meeting_id.into());
    window.get_active_meeting_id() == meeting_id
}

fn resume(window: &MainWindow, handle: &AppHandle, meeting_id: &str) {
    close_stale_meeting_view(window, handle, meeting_id);
    if window.get_recording_mode() != RecordingMode::Idle {
        // A dictation the user started since: the detail shows the meeting
        // with its Resume button once it ends.
        fall_back_to_manual_resume(window, handle, meeting_id);
        return;
    }
    if !open_meeting(window, meeting_id) {
        return;
    }
    WAKE_RESUME.with(|slot| slot.borrow_mut().resuming = Some(meeting_id.to_string()));
    window.invoke_meeting_detail_resume();
    if window.get_recording_mode() != RecordingMode::Meeting {
        // The handler refused before starting anything.
        WAKE_RESUME.with(|slot| slot.borrow_mut().resuming = None);
        fall_back_to_manual_resume(window, handle, meeting_id);
    }
}

fn fall_back_to_manual_resume(window: &MainWindow, handle: &AppHandle, meeting_id: &str) {
    close_stale_meeting_view(window, handle, meeting_id);
    WAKE_RESUME.with(|slot| slot.borrow_mut().fallback = Some(meeting_id.to_string()));
    // Opened under a recording, the detail shows once the recording ends.
    if open_meeting(window, meeting_id) {
        show_notice(window, WakeResumeNotice::Interrupted);
    }
}

fn show_notice(window: &MainWindow, notice: WakeResumeNotice) {
    window.set_wake_resume_notice(notice);
    let weak = window.as_weak();
    WAKE_RESUME.with(|slot| {
        slot.borrow().notice_timer.start(
            slint::TimerMode::SingleShot,
            NOTICE_AUTO_HIDE,
            move || {
                if let Some(window) = weak.upgrade()
                    && window.get_wake_resume_notice() == notice
                {
                    window.set_wake_resume_notice(WakeResumeNotice::Hidden);
                }
            },
        );
    });
}

/// The notice's X.
pub fn dismiss_notice(window: &MainWindow) {
    WAKE_RESUME.with(|slot| slot.borrow().notice_timer.stop());
    window.set_wake_resume_notice(WakeResumeNotice::Hidden);
}

/// The detail's Resume handler is about to start a resume (the user's or a
/// wake's): the notice about the previous state is moot.
pub fn resume_requested(window: &MainWindow, meeting_id: &str) {
    WAKE_RESUME.with(|slot| {
        let mut state = slot.borrow_mut();
        if state.fallback.as_deref() == Some(meeting_id) {
            state.fallback = None;
        }
    });
    dismiss_notice(window);
}

/// The detail's Resume handler settled. A wake-resume that failed leaves the
/// meeting open on its error, like the fallback.
pub fn resume_settled(window: &MainWindow, meeting_id: &str, resumed: bool) {
    let was_wake = WAKE_RESUME.with(|slot| {
        let mut state = slot.borrow_mut();
        let was_wake = state.resuming.as_deref() == Some(meeting_id);
        if was_wake {
            state.resuming = None;
            if !resumed {
                state.fallback = Some(meeting_id.to_string());
            }
        }
        was_wake
    });
    if was_wake && resumed {
        show_notice(window, WakeResumeNotice::Resumed);
    }
}

/// The detail was left (Back or Delete). Leaving the fallback without
/// resuming is a refusal: a later wake must not offer that meeting again.
pub fn detail_closed(window: &MainWindow, handle: &AppHandle, meeting_id: &str) {
    let refused = WAKE_RESUME.with(|slot| {
        let mut state = slot.borrow_mut();
        let refused = state.fallback.as_deref() == Some(meeting_id);
        if refused {
            state.fallback = None;
        }
        refused
    });
    if refused {
        handle.clear_sleep_paused_meeting();
    }
    if window.get_wake_resume_notice() == WakeResumeNotice::Interrupted {
        dismiss_notice(window);
    }
}

/// The recording view closed: "Recording resumed" is about it.
pub fn recording_ended(window: &MainWindow) {
    if window.get_wake_resume_notice() == WakeResumeNotice::Resumed {
        dismiss_notice(window);
    }
}

#[cfg(test)]
mod tests {
    use super::{WakeResumeStep, wake_resume_step};
    use souffle_lib::engine::TranscriptionProfile;
    use souffle_lib::state_machine::{AppStateMachine, RecordingKind};

    fn profile() -> TranscriptionProfile {
        TranscriptionProfile::default()
    }

    fn paused() -> Option<String> {
        Some("meeting-1".into())
    }

    #[test]
    fn nothing_to_resume_without_a_meeting_stopped_by_sleep() {
        let ready = AppStateMachine::Ready { profile: profile() };
        assert_eq!(
            wake_resume_step(None, &ready, false),
            WakeResumeStep::Nothing
        );
        assert_eq!(
            wake_resume_step(None, &AppStateMachine::Idle, true),
            WakeResumeStep::Nothing
        );
    }

    #[test]
    fn a_ready_machine_resumes_the_meeting_the_sleep_stopped() {
        let ready = AppStateMachine::Ready { profile: profile() };
        assert_eq!(
            wake_resume_step(paused(), &ready, false),
            WakeResumeStep::Resume("meeting-1".into())
        );
        // The resume path loads an unloaded model itself.
        assert_eq!(
            wake_resume_step(paused(), &AppStateMachine::Idle, false),
            WakeResumeStep::Resume("meeting-1".into())
        );
    }

    #[test]
    fn a_stop_still_draining_at_wake_is_waited_for() {
        let draining = AppStateMachine::Stopping {
            profile: profile(),
            was_recording: RecordingKind::Meeting {
                meeting_id: "meeting-1".into(),
            },
        };
        assert_eq!(
            wake_resume_step(paused(), &draining, false),
            WakeResumeStep::Wait("meeting-1".into())
        );
        let not_yet_stopped = AppStateMachine::RecordingMeeting {
            profile: profile(),
            session_id: 1,
            meeting_id: "meeting-1".into(),
        };
        assert_eq!(
            wake_resume_step(paused(), &not_yet_stopped, false),
            WakeResumeStep::Wait("meeting-1".into())
        );
        let loading = AppStateMachine::Loading { profile: profile() };
        assert_eq!(
            wake_resume_step(paused(), &loading, false),
            WakeResumeStep::Wait("meeting-1".into())
        );
    }

    #[test]
    fn a_start_the_user_asked_for_is_not_overridden() {
        let ready = AppStateMachine::Ready { profile: profile() };
        assert_eq!(
            wake_resume_step(paused(), &ready, true),
            WakeResumeStep::Wait("meeting-1".into())
        );
    }
}
