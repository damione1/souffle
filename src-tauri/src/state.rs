use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};

use crossbeam_channel::Sender;
use tauri::AppHandle;
use tauri_specta::Event;
use tracing::debug;

use crate::app_events::StateChanged;
use crate::db::Database;
use crate::engine::{TranscriptionProfile, TranscriptionSegment};
use crate::lock_ext::MutexExt;
use crate::pipeline::EngineActorHandle;
use crate::state_machine::{AppStateMachine, StateAction};
use crate::transcript::MeetingRecordingSession;

/// Commands sent to the audio thread
pub enum AudioCommand {
    Start {
        session_id: u64,
        target_sample_rate: u32,
        mic_gain: f32,
        /// Meeting mode: also capture system audio via a Core Audio tap
        /// and mix it with the microphone.
        capture_system_audio: bool,
        /// Diarized meeting: emit the mic (Me) and system audio (Them) as two
        /// source-tagged streams instead of one mixed stream, so each can be
        /// transcribed by its own engine. Only meaningful with system audio.
        diarize: bool,
    },
    Stop,
    SelectDevice(String),
    /// Give the audio thread an AppHandle so meeting mode can emit
    /// SystemAudioStatus events.
    AttachApp(AppHandle),
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
    /// Notes the user types while the meeting records; persisted at stop.
    pub notes: Option<String>,
    /// How many of `new_segments` have already been flushed to the DB by the
    /// incremental persistence path. Lets a crash mid-meeting lose at most the
    /// last unflushed batch instead of the whole session.
    pub persisted_new_count: usize,
}

/// Shared application state, managed by Tauri.
/// AudioCapture lives on its own thread (cpal Stream is !Send on macOS),
/// and the transcription engine lives on the engine actor thread —
/// both are driven via command channels.
pub struct AppState {
    pub audio_cmd_sender: Sender<AudioCommand>,
    /// Wrapped in Arc so async commands can clone a handle into a blocking
    /// task (`spawn_blocking`) and keep the long crossbeam reply waits off the
    /// Tauri command thread — otherwise they freeze the window event loop.
    pub engine_actor: Arc<EngineActorHandle>,
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
        engine_actor: Arc<EngineActorHandle>,
        db: Arc<Database>,
        audio_rms: Arc<AtomicU32>,
    ) -> Self {
        Self {
            audio_cmd_sender,
            engine_actor,
            next_audio_session_id: Mutex::new(0),
            meeting_accumulator: Arc::new(Mutex::new(None)),
            db,
            audio_rms,
            machine: Mutex::new(AppStateMachine::Idle),
            app_handle: Mutex::new(None),
        }
    }

    /// Apply a state transition, update the machine, and emit a StateChanged event.
    pub fn apply_transition(&self, action: StateAction) -> Result<AppStateMachine, String> {
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
            crate::pill::sync(handle, &new_state);
            crate::tray::sync(handle, &new_state);
        }

        Ok(new_state)
    }

    /// Get a clone of the current machine state.
    pub fn current_machine_state(&self) -> Result<AppStateMachine, String> {
        Ok(self.machine.acquire()?.clone())
    }

    /// Clone the stored AppHandle (set during setup). Used by async commands to
    /// drive background finalization tasks that re-fetch `AppState` off-thread.
    pub fn app_handle(&self) -> Result<AppHandle, String> {
        self.app_handle
            .acquire()?
            .as_ref()
            .cloned()
            .ok_or_else(|| "App handle not set".to_string())
    }
}
