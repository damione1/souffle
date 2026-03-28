use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender};
use tauri::AppHandle;
use tauri_specta::Event;
use tracing::debug;

use crate::app_events::StateChanged;
use crate::audio::AudioChunk;
use crate::db::Database;
use crate::engine::{
    SharedTranscriptionEngine, TranscriptionProfile, TranscriptionSegment,
    default_transcription_engine,
};
use crate::lock_ext::MutexExt;
use crate::pipeline::TranscriptionPipeline;
use crate::state_machine::{AppStateMachine, StateAction};
use crate::transcript::MeetingRecordingSession;

/// Commands sent to the audio thread
pub enum AudioCommand {
    Start(u64),
    Stop,
    SelectDevice(String),
}

/// Accumulated meeting segments while recording
pub struct MeetingAccumulator {
    pub id: String,
    pub title: String,
    pub existing_segments: Vec<TranscriptionSegment>,
    pub new_segments: Vec<TranscriptionSegment>,
    pub recording_sessions: Vec<MeetingRecordingSession>,
    pub session_started_at: chrono::DateTime<chrono::Utc>,
    pub transcription_profile: TranscriptionProfile,
    pub summary: Option<String>,
    pub summary_is_stale: bool,
    pub summary_model: Option<String>,
    pub summary_generated_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Shared application state, managed by Tauri.
/// AudioCapture lives on its own thread (cpal Stream is !Send on macOS),
/// so we communicate with it via a command channel.
pub struct AppState {
    pub audio_cmd_sender: Sender<AudioCommand>,
    pub audio_receiver: Receiver<AudioChunk>,
    pub engine: SharedTranscriptionEngine,
    pub pipeline: Mutex<Option<TranscriptionPipeline>>,
    pub next_audio_session_id: Mutex<u64>,
    pub meeting_accumulator: Arc<Mutex<Option<MeetingAccumulator>>>,
    pub db: Arc<Database>,
    /// Latest audio RMS level (0.0-1.0), stored as f32 bits in AtomicU32
    pub audio_rms: Arc<AtomicU32>,
    /// Unified state machine — the source of truth for app lifecycle
    pub machine: Mutex<AppStateMachine>,
    /// Tauri app handle for emitting events (set during setup)
    pub app_handle: Mutex<Option<AppHandle>>,
}

impl AppState {
    pub fn new(
        audio_cmd_sender: Sender<AudioCommand>,
        audio_receiver: Receiver<AudioChunk>,
        db: Arc<Database>,
        audio_rms: Arc<AtomicU32>,
    ) -> Self {
        Self {
            audio_cmd_sender,
            audio_receiver,
            engine: Arc::new(Mutex::new(default_transcription_engine())),
            pipeline: Mutex::new(None),
            next_audio_session_id: Mutex::new(0),
            meeting_accumulator: Arc::new(Mutex::new(None)),
            db,
            audio_rms,
            machine: Mutex::new(AppStateMachine::Idle),
            app_handle: Mutex::new(None),
        }
    }

    /// Apply a state transition, update the machine, and emit a StateChanged event.
    pub fn apply_transition(
        &self,
        action: StateAction,
    ) -> Result<AppStateMachine, String> {
        let mut machine = self.machine.acquire()?;
        let new_state = machine.clone().transition(action)?;
        debug!(
            from = machine.variant_name(),
            to = new_state.variant_name(),
            "State transition"
        );
        *machine = new_state.clone();

        // Emit event to frontend if app_handle is available
        if let Ok(handle_guard) = self.app_handle.lock()
            && let Some(ref handle) = *handle_guard
        {
            let _ = StateChanged(new_state.clone()).emit(handle);
        }

        Ok(new_state)
    }

    /// Get a clone of the current machine state.
    pub fn current_machine_state(&self) -> Result<AppStateMachine, String> {
        Ok(self.machine.acquire()?.clone())
    }
}
