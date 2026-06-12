use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

use crate::state_machine::AppStateMachine;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(rename_all = "kebab-case")]
pub enum AppView {
    Transcription,
    Meeting,
    MeetingHistory,
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

/// State of the system-audio capture leg of a meeting session, emitted when
/// the session starts and whenever the leg changes (e.g. tap rebuild after
/// an output device switch).
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct SystemAudioStatus {
    pub active: bool,
    /// Present when inactive because of an error (e.g. permission denied).
    pub reason: Option<String>,
}
