use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender};

use crate::audio::AudioChunk;
use crate::db::Database;
use crate::engine::TranscriptionSegment;
use crate::engine::kyutai::KyutaiEngine;
use crate::pipeline::TranscriptionPipeline;

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
    pub title: String,
    pub segments: Vec<TranscriptionSegment>,
    pub started_at: chrono::DateTime<chrono::Utc>,
}

/// Shared application state, managed by Tauri.
/// AudioCapture lives on its own thread (cpal Stream is !Send on macOS),
/// so we communicate with it via a command channel.
pub struct AppState {
    pub audio_cmd_sender: Sender<AudioCommand>,
    pub audio_receiver: Receiver<AudioChunk>,
    pub is_recording: Mutex<bool>,
    pub engine: Arc<Mutex<KyutaiEngine>>,
    pub pipeline: Mutex<Option<TranscriptionPipeline>>,
    pub model_loaded: Mutex<bool>,
    pub next_audio_session_id: Mutex<u64>,
    pub recording_mode: Mutex<RecordingMode>,
    pub meeting_accumulator: Arc<Mutex<Option<MeetingAccumulator>>>,
    pub db: Arc<Database>,
}

impl AppState {
    pub fn new(
        audio_cmd_sender: Sender<AudioCommand>,
        audio_receiver: Receiver<AudioChunk>,
        db: Arc<Database>,
    ) -> Self {
        Self {
            audio_cmd_sender,
            audio_receiver,
            is_recording: Mutex::new(false),
            engine: Arc::new(Mutex::new(KyutaiEngine::new())),
            pipeline: Mutex::new(None),
            model_loaded: Mutex::new(false),
            next_audio_session_id: Mutex::new(0),
            recording_mode: Mutex::new(RecordingMode::Idle),
            meeting_accumulator: Arc::new(Mutex::new(None)),
            db,
        }
    }
}
