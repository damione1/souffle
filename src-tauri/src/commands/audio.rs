use tauri::State;

use crate::audio::capture::{AudioDeviceInfo, list_input_devices};
use crate::state::{AppState, AudioCommand};

/// List available audio input devices
#[tauri::command]
#[specta::specta]
pub fn list_audio_devices() -> Result<Vec<AudioDeviceInfo>, String> {
    Ok(list_input_devices())
}

/// Select an audio input device by name
#[tauri::command]
#[specta::specta]
pub fn select_audio_device(state: State<'_, AppState>, device_name: String) -> Result<(), String> {
    state
        .audio_cmd_sender
        .send(AudioCommand::SelectDevice(device_name))
        .map_err(|e| format!("Failed to send device selection: {e}"))
}

/// Get the current audio input level (RMS, 0.0–1.0) for waveform visualization
#[tauri::command]
#[specta::specta]
pub fn get_audio_level(state: State<'_, AppState>) -> Result<f32, String> {
    Ok(f32::from_bits(
        state.audio_rms.load(std::sync::atomic::Ordering::Relaxed),
    ))
}
