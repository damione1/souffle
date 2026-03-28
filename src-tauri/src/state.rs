use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender};

use crate::audio::AudioChunk;
use crate::db::Database;
use crate::engine::{
    SharedTranscriptionEngine, TranscriptionProfile, TranscriptionSegment,
    default_transcription_engine,
};
use crate::pipeline::TranscriptionPipeline;
use crate::transcript::MeetingRecordingSession;

/// Commands sent to the audio thread
pub enum AudioCommand {
    Start(u64),
    Stop,
    SelectDevice(String),
}

/// Whether we're in dictation or meeting recording mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecordingMode {
    Idle,
    Dictation,
    Meeting,
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
    pub is_recording: Mutex<bool>,
    pub engine: SharedTranscriptionEngine,
    pub pipeline: Mutex<Option<TranscriptionPipeline>>,
    pub model_loaded: Mutex<bool>,
    pub active_profile: Mutex<Option<TranscriptionProfile>>,
    pub next_audio_session_id: Mutex<u64>,
    pub recording_mode: Mutex<RecordingMode>,
    pub meeting_accumulator: Arc<Mutex<Option<MeetingAccumulator>>>,
    pub db: Arc<Database>,
    /// Latest audio RMS level (0.0-1.0), stored as f32 bits in AtomicU32
    pub audio_rms: Arc<AtomicU32>,
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
            is_recording: Mutex::new(false),
            engine: Arc::new(Mutex::new(default_transcription_engine())),
            pipeline: Mutex::new(None),
            model_loaded: Mutex::new(false),
            active_profile: Mutex::new(None),
            next_audio_session_id: Mutex::new(0),
            recording_mode: Mutex::new(RecordingMode::Idle),
            meeting_accumulator: Arc::new(Mutex::new(None)),
            db,
            audio_rms,
        }
    }
}
