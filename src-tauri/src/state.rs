use std::sync::Mutex;

use crossbeam_channel::{Receiver, Sender};

/// Commands sent to the audio thread
pub enum AudioCommand {
    Start,
    Stop,
}

/// Shared application state, managed by Tauri.
/// AudioCapture lives on its own thread (cpal Stream is !Send on macOS),
/// so we communicate with it via a command channel.
pub struct AppState {
    pub audio_cmd_sender: Sender<AudioCommand>,
    pub audio_receiver: Receiver<Vec<f32>>,
    pub is_recording: Mutex<bool>,
}

impl AppState {
    pub fn new(
        audio_cmd_sender: Sender<AudioCommand>,
        audio_receiver: Receiver<Vec<f32>>,
    ) -> Self {
        Self {
            audio_cmd_sender,
            audio_receiver,
            is_recording: Mutex::new(false),
        }
    }
}
