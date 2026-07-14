use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

use crate::state_machine::AppStateMachine;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(rename_all = "kebab-case")]
pub enum AppView {
    Home,
    Settings,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct Navigate(pub AppView);

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct ShortcutToggle;

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct ShortcutPttStart;

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct ShortcutPttStop;

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct StateChanged(pub AppStateMachine);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Healthy,
    /// Inference is behind real-time, or audio chunks were dropped.
    Lagging,
    /// No frame has been processed for several seconds while audio is queued.
    Stalled,
}

/// Periodic pipeline health snapshot emitted during recording sessions.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum PipelineErrorScope {
    /// A single frame failed to transcribe and was skipped.
    Frame,
    /// The session was aborted (e.g. repeated engine failures).
    Session,
}

/// Pipeline failure surfaced to the frontend instead of dying silently in logs.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct PipelineError {
    pub scope: PipelineErrorScope,
    pub message: String,
}

/// Emitted by the floating recording pill (or the tray) to ask the meeting
/// controller in the main window to stop the active meeting through its
/// normal stop pipeline.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct MeetingStopRequested;

/// Emitted once a stopped meeting has been fully drained and saved in the
/// background, so the detail view can refresh from the now-complete record.
/// `stop_meeting_recording` returns before this work finishes (decoupled stop),
/// so the UI shows the partially-persisted meeting immediately and reconciles
/// when this arrives.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct MeetingFinalized {
    pub id: String,
}

/// State of the system-audio capture leg of a meeting session, emitted when
/// the session starts and whenever the leg changes (e.g. tap rebuild after
/// an output device switch).
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct SystemAudioStatus {
    pub active: bool,
    /// Present when inactive because of an error (e.g. permission denied).
    pub reason: Option<String>,
}

/// Current microphone/meeting input level (RMS, 0.0-1.0), pushed by the audio
/// thread while a capture session is active so the waveform UI doesn't need
/// to poll `get_audio_level` over IPC.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, Event)]
pub struct AudioLevel {
    pub level: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum CalendarMeetingNudgeKind {
    /// Pre-event reminder inside the configured lead-time window.
    #[default]
    Reminder,
    /// At-event-time suggestion when system audio is active but no recording runs.
    Autostart,
}

/// Emitted by the calendar reminder scheduler shortly before a calendar
/// event starts, so the frontend can offer a one-click transcription start.
/// A system notification is sent alongside; this event drives the in-app
/// banner (notification clicks are not reliably delivered on macOS).
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct UpcomingMeeting {
    pub event: crate::calendar::CalendarEvent,
    pub starts_in_seconds: u32,
    #[serde(default)]
    pub kind: CalendarMeetingNudgeKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum MeetingIdleReason {
    /// No transcript segment with text for the configured silence threshold.
    Silence,
    /// The session has run past the configured max-duration failsafe.
    MaxDuration,
}

/// A meeting recording session looks like it has ended: either speech has
/// stopped for a while, or the session hit the max-duration failsafe. Drives
/// the live-card banner; a system notification is also sent on the first
/// occurrence (see `pipeline::idle::MeetingIdleSignal::first`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, Event)]
pub struct MeetingIdle {
    pub reason: MeetingIdleReason,
    pub idle_seconds: u64,
    pub threshold_seconds: u64,
}

/// Progress for a full data archive export (`commands::data::export_archive`),
/// which runs on a background thread so the command itself returns
/// immediately. Emitted once per meeting processed, plus a final event with
/// `finished: true`. `error` carries a fatal, whole-archive failure (e.g. the
/// destination became unwritable); a single bad meeting is not fatal, it is
/// only reflected in the written manifest's `errors` count.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct ArchiveExportProgress {
    pub done: u32,
    pub total: u32,
    pub finished: bool,
    pub error: Option<String>,
}

/// A background diarization pass finished labeling a mic-only meeting's
/// segments with persistent speaker identities. The frontend reloads the
/// meeting if it's currently open, so the labels appear without a manual
/// refresh. Only ever emitted after `MeetingFinalized` for the same meeting
/// (diarization runs strictly after the transcript is saved), and only when
/// at least one segment was actually relabeled.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct MeetingDiarized {
    pub meeting_id: String,
}

/// Coarse progress for the background diarization pass over one meeting's
/// recorded sessions (same shape as `ArchiveExportProgress`): one event when
/// the pass starts, one per session completed, and a final event with
/// `finished: true`. `error` carries a whole-pass failure; a single bad
/// session is non-fatal and only logged.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct DiarizationProgress {
    pub meeting_id: String,
    pub done_sessions: u32,
    pub total_sessions: u32,
    pub finished: bool,
    pub error: Option<String>,
}

/// The system finished sleeping and woke back up (`NSWorkspaceDidWakeNotification`).
/// The frontend calls `take_sleep_paused_meeting` on receiving this (and again
/// on webview visibility change, in case the webview itself was suspended
/// when this fired) to see whether a meeting was paused by sleep and, if so,
/// offer to resume it.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct SystemWokeUp;

/// Reasons the floating pill must stay visible even though the state machine
/// already left a recording state. Only "polishing" today: dictation's
/// optional LLM reformulation pass runs for a few seconds after the
/// transcript is finalized, and the user should see that it's still working
/// before the paste lands.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum PillHoldKind {
    Polishing,
}

/// Frontend-driven hold on the pill window, toggled by the `pill_hold` /
/// `pill_release` commands. The pill runs in its own webview, separate from
/// whatever calls those commands, so it needs this event to know when to
/// render the hold-specific state (e.g. "Reformulating...").
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, Event)]
pub struct PillHoldChanged {
    pub kind: Option<PillHoldKind>,
}

/// Throttled tail of the current dictation transcript (final segments only,
/// space-joined like the main window's assembled transcript), so the
/// floating pill — a separate webview — can show what's being said without
/// piggy-backing on the main window's segment channel. Dictation only, never
/// emitted for meetings. An empty `text` marks the end of the session.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct DictationLiveText {
    pub text: String,
}

/// CoreAudio reported a new input-device snapshot (connect/disconnect or
/// default-input change). Settings listens to refresh the microphone list.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct InputDevicesChanged {
    pub devices: Vec<crate::audio::AudioInputDevice>,
}

/// The user pinned an input device that is not currently connected. Capture
/// falls back through the priority policy without clearing the saved pin.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct InputPinUnavailable {
    pub uid: String,
}

/// A previously unavailable pinned input device is connected again.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct InputPinAvailable {
    pub uid: String,
}
