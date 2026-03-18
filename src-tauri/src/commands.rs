use std::path::PathBuf;
use std::time::Duration;

use tauri::State;
use tracing::info;

use crate::state::{AppState, AudioCommand};

/// Start recording audio from the default microphone
#[tauri::command]
pub fn start_recording(state: State<'_, AppState>) -> Result<(), String> {
    let mut is_recording = state
        .is_recording
        .lock()
        .map_err(|e| format!("Lock error: {e}"))?;

    if *is_recording {
        return Err("Already recording".into());
    }

    state
        .audio_cmd_sender
        .send(AudioCommand::Start)
        .map_err(|e| format!("Failed to send start command: {e}"))?;

    *is_recording = true;
    info!("Recording started");
    Ok(())
}

/// Stop recording and return the path to the saved WAV file
#[tauri::command]
pub fn stop_recording(state: State<'_, AppState>) -> Result<String, String> {
    let mut is_recording = state
        .is_recording
        .lock()
        .map_err(|e| format!("Lock error: {e}"))?;

    if !*is_recording {
        return Err("Not recording".into());
    }

    state
        .audio_cmd_sender
        .send(AudioCommand::Stop)
        .map_err(|e| format!("Failed to send stop command: {e}"))?;

    *is_recording = false;

    // Give cpal a moment to flush its buffers
    std::thread::sleep(Duration::from_millis(200));

    // Drain all audio from the channel
    let mut all_samples = Vec::new();
    while let Ok(chunk) = state.audio_receiver.try_recv() {
        all_samples.extend_from_slice(&chunk);
    }

    if all_samples.is_empty() {
        info!("Recording stopped - no audio captured");
        return Ok("No audio captured".into());
    }

    let wav_path = save_wav(&all_samples)?;
    let path_str = wav_path.display().to_string();
    info!(path = %path_str, samples = all_samples.len(), "Recording saved");

    Ok(format!(
        "Recorded {:.1}s of audio → {}",
        all_samples.len() as f64 / 16_000.0,
        path_str
    ))
}

/// Save f32 PCM samples (16kHz mono) to a WAV file
fn save_wav(samples: &[f32]) -> Result<PathBuf, String> {
    let recordings_dir = dirs_next::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.souffle.app")
        .join("recordings");

    std::fs::create_dir_all(&recordings_dir)
        .map_err(|e| format!("Failed to create recordings dir: {e}"))?;

    let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    let filename = format!("{timestamp}.wav");
    let path = recordings_dir.join(&filename);

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let mut writer = hound::WavWriter::create(&path, spec)
        .map_err(|e| format!("Failed to create WAV: {e}"))?;

    for &sample in samples {
        writer
            .write_sample(sample)
            .map_err(|e| format!("Failed to write sample: {e}"))?;
    }

    writer
        .finalize()
        .map_err(|e| format!("Failed to finalize WAV: {e}"))?;

    Ok(path)
}
