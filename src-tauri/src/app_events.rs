//! Value types that used to double as `tauri_specta` IPC events.
//!
//! Before SOU-191 every one of these was also emitted to the webview via
//! `tauri_specta::Event`. Grepped project-wide: there is no `.listen(` call
//! anywhere in this tree (the Slint shell calls functions directly instead
//! of listening for events), so every emit was already inert dead code by
//! the time this ticket started — this file keeps the value types (several
//! are still real return values / DB payloads) and drops only the IPC
//! wiring (`specta::Type`, `tauri_specta::Event`) that required Tauri.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum HealthStatus {
    Healthy,
    /// Inference is behind real-time, or audio chunks were dropped.
    Lagging,
    /// No frame has been processed for several seconds while audio is queued.
    Stalled,
}

/// Periodic pipeline health snapshot (formerly emitted during recording
/// sessions; kept as a value type for `pipeline`'s own bookkeeping).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionHealth {
    pub session_id: u32,
    pub status: HealthStatus,
    /// Audio chunks waiting in the capture→inference channel.
    pub queue_depth: u32,
    /// Age of the most-delayed chunk processed in the last window (ms).
    pub lag_ms: u32,
    /// Chunks dropped by the capture callback since the session started.
    pub dropped_chunks: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PipelineErrorScope {
    /// A single frame failed to transcribe and was skipped.
    Frame,
    /// The session was aborted (e.g. repeated engine failures).
    Session,
}

/// Pipeline failure (formerly surfaced to the frontend; kept for logging /
/// future in-app surfacing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineError {
    pub scope: PipelineErrorScope,
    pub message: String,
}

/// Why the system-audio leg of a meeting is not carrying the other
/// participants. Machine-readable so the UI can say it in the user's
/// language instead of showing a raw CoreAudio error (SOU-119).
///
/// The order of the variants is the order of severity used by
/// [`worse_system_audio`]: later means "less of a system-audio leg".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemAudioReason {
    /// The tap delivered audio all session, and every sample was silence.
    Silent,
    /// The tap ran without ever delivering a frame: its dispatch queue never
    /// fired, which is not the same as delivering silence.
    NoSamples,
    /// The tap was lost mid-session and could not be started again.
    TapLost,
    /// The capture session failed to start after the tap was acquired (e.g.
    /// the microphone went away), taking the tap down with it.
    StartFailed,
    /// The tap probe run before the session started failed (SOU-082).
    ProbeFailed,
    /// Reserved for a verified TCC denial. CreateProcessTap failing is not
    /// that: a wedged coreaudiod returns the same error while permission is
    /// still granted. Do not assign this from an error-string prefix.
    PermissionDenied,
    /// `capture_system_audio` is off in the settings, so no tap was tried.
    Disabled,
    /// The OS is too old for process taps (macOS < 14.4, or not macOS).
    Unsupported,
}

/// State of the system-audio capture leg of a meeting session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemAudioStatus {
    pub active: bool,
    /// Present when inactive because of an error (e.g. permission denied).
    pub reason: Option<String>,
    /// Why the leg is inactive, when that is known.
    pub reason_code: Option<SystemAudioReason>,
    /// Frames the leg has delivered so far this session, silence included.
    /// Zero while `active` is true means the tap's queue never fired.
    pub samples: u64,
    /// Of those, how many carried far-end signal. A tap on the device clock
    /// delivers frames whether or not anything plays, so this, not `samples`,
    /// is what says the other participants were actually heard (SOU-119).
    pub signal_samples: u64,
}

/// What the system-audio leg did over a recording. Kept across resumes by
/// [`worse_system_audio`], so this is the *worst* verdict any of the
/// meeting's sessions reached, and `samples` / `signal_samples` are the ones
/// measured during that session, not a total over the meeting (SOU-119).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingSystemAudio {
    pub active: bool,
    pub reason: Option<String>,
    pub reason_code: Option<SystemAudioReason>,
    pub samples: u64,
    pub signal_samples: u64,
}

impl MeetingSystemAudio {
    /// True when the meeting did not actually get the other participants:
    /// either the leg never ran, or it ran without ever carrying signal.
    pub fn degraded(&self) -> bool {
        !self.active || self.signal_samples == 0
    }

    /// How little of a system-audio leg this verdict describes. Only used to
    /// order two verdicts against each other.
    fn severity(&self) -> (u8, Option<SystemAudioReason>) {
        (u8::from(self.degraded()), self.reason_code)
    }
}

/// Keep the verdict that explains the most missing system audio. A meeting
/// can be stopped and resumed, and each session produces its own verdict; a
/// healthy 30s resume must not erase the hour that was recorded mic only,
/// and between two degraded sessions the one whose leg was missing the most
/// (the highest `SystemAudioReason`) wins. Ties keep the earlier verdict.
pub fn worse_system_audio(
    previous: Option<MeetingSystemAudio>,
    current: Option<MeetingSystemAudio>,
) -> Option<MeetingSystemAudio> {
    match (previous, current) {
        (None, current) => current,
        (Some(previous), None) => Some(previous),
        (Some(previous), Some(current)) => {
            if current.severity() > previous.severity() {
                Some(current)
            } else {
                Some(previous)
            }
        }
    }
}

/// Whether the native single-key PTT `CGEventTap` is installed. Read by
/// `get_modifier_tap_status` (SOU-116); no longer edge-triggered over IPC,
/// just a plain snapshot.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ModifierTapStatus {
    pub installed: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CalendarMeetingNudgeKind {
    /// Pre-event reminder inside the configured lead-time window.
    #[default]
    Reminder,
    /// At-event-time suggestion when system audio is active but no recording runs.
    Autostart,
}

/// Emitted by the calendar reminder scheduler shortly before a calendar
/// event starts. A system notification is sent alongside via
/// `native::notifications`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpcomingMeeting {
    pub event: crate::calendar::CalendarEvent,
    pub starts_in_seconds: u32,
    #[serde(default)]
    pub kind: CalendarMeetingNudgeKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MeetingIdleReason {
    /// No transcript segment with text for the configured silence threshold.
    Silence,
    /// The session has run past the configured max-duration failsafe.
    MaxDuration,
}

/// A meeting recording session looks like it has ended: either speech has
/// stopped for a while, or the session hit the max-duration failsafe. A
/// system notification is also sent on the first occurrence (see
/// `pipeline::idle::MeetingIdleSignal::first`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MeetingIdle {
    pub reason: MeetingIdleReason,
    pub idle_seconds: u64,
    pub threshold_seconds: u64,
}

/// Why an input-route toast was emitted.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InputRouteReason {
    /// Resolved capture target changed.
    Switched,
    /// New device appeared but is not the capture target.
    Connected,
    /// Capture device gone (and maybe no replacement yet).
    Lost,
}

/// Live microphone-route change, formerly a FluidVoice-style toast.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InputRouteNotice {
    pub reason: InputRouteReason,
    pub from_name: Option<String>,
    pub to_name: Option<String>,
    pub to_uid: Option<String>,
    pub transport: Option<crate::audio::TransportType>,
}

/// Reasons the floating pill must stay visible even though the state machine
/// already left a recording state. Only "polishing" today: dictation's
/// optional LLM reformulation pass runs for a few seconds after the
/// transcript is finalized, and the user should see that it's still working
/// before the paste lands.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PillHoldKind {
    Polishing,
}

/// Phase of the in-app updater download pipeline (not the app state machine).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePhase {
    Idle,
    Downloading,
    Ready,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict(reason_code: Option<SystemAudioReason>, signal_samples: u64) -> MeetingSystemAudio {
        MeetingSystemAudio {
            active: reason_code.is_none() || signal_samples > 0,
            reason: None,
            reason_code,
            samples: 480_000,
            signal_samples,
        }
    }

    fn healthy() -> MeetingSystemAudio {
        verdict(None, 96_000)
    }

    #[test]
    fn a_leg_that_carried_signal_is_not_degraded() {
        assert!(!healthy().degraded());
        assert!(verdict(Some(SystemAudioReason::Silent), 0).degraded());
        assert!(verdict(Some(SystemAudioReason::Disabled), 0).degraded());
    }

    /// SOU-119: a meeting resumed for 30 healthy seconds must not lose the
    /// hour it recorded mic only.
    #[test]
    fn a_healthy_resume_does_not_erase_a_degraded_session() {
        let kept = worse_system_audio(
            Some(verdict(Some(SystemAudioReason::ProbeFailed), 0)),
            Some(healthy()),
        )
        .unwrap();
        assert_eq!(kept.reason_code, Some(SystemAudioReason::ProbeFailed));
    }

    /// And the symmetric case: a healthy hour resumed with the setting off
    /// must end up labelled degraded.
    #[test]
    fn a_degraded_resume_overrides_a_healthy_session() {
        let kept = worse_system_audio(
            Some(healthy()),
            Some(verdict(Some(SystemAudioReason::Disabled), 0)),
        )
        .unwrap();
        assert_eq!(kept.reason_code, Some(SystemAudioReason::Disabled));
    }

    #[test]
    fn the_more_complete_absence_wins_between_two_degraded_sessions() {
        let kept = worse_system_audio(
            Some(verdict(Some(SystemAudioReason::Silent), 0)),
            Some(verdict(Some(SystemAudioReason::PermissionDenied), 0)),
        )
        .unwrap();
        assert_eq!(kept.reason_code, Some(SystemAudioReason::PermissionDenied));

        let kept = worse_system_audio(
            Some(verdict(Some(SystemAudioReason::PermissionDenied), 0)),
            Some(verdict(Some(SystemAudioReason::Silent), 0)),
        )
        .unwrap();
        assert_eq!(
            kept.reason_code,
            Some(SystemAudioReason::PermissionDenied),
            "a milder later session must not replace a more severe one"
        );
    }

    #[test]
    fn a_missing_side_leaves_the_other_untouched() {
        assert!(worse_system_audio(None, None).is_none());
        assert_eq!(
            worse_system_audio(None, Some(healthy()))
                .unwrap()
                .reason_code,
            None
        );
        let kept = worse_system_audio(Some(verdict(Some(SystemAudioReason::TapLost), 0)), None);
        assert_eq!(kept.unwrap().reason_code, Some(SystemAudioReason::TapLost));
    }
}
