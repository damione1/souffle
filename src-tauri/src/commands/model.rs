use tauri::State;
use tauri::ipc::Channel;
use tracing::info;

use crate::engine::TranscriptionEngine;
use crate::lock_ext::MutexExt;
use crate::models;
use crate::pipeline::TranscriptionPipeline;
use crate::state::AppState;
use std::sync::Arc;

/// Status of the STT model
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelStatus {
    pub downloaded: bool,
    pub loaded: bool,
    pub model_dir: String,
    pub engine_name: String,
}

/// Check whether the model is downloaded and loaded
#[tauri::command]
pub fn get_model_status(state: State<'_, AppState>) -> Result<ModelStatus, String> {
    let model_dir = models::default_model_dir();
    let downloaded = models::model_exists(&model_dir);
    let loaded = *state.model_loaded.acquire()?;
    let engine_name = state.engine.acquire()?.name().to_string();

    Ok(ModelStatus {
        downloaded,
        loaded,
        model_dir: model_dir.display().to_string(),
        engine_name,
    })
}

/// Download the Kyutai STT model from HuggingFace.
/// Progress is streamed back via the Channel API.
#[tauri::command]
pub fn download_model(channel: Channel<models::DownloadProgress>) -> Result<(), String> {
    let model_dir = models::default_model_dir();

    if models::model_exists(&model_dir) {
        channel
            .send(models::DownloadProgress {
                file: "all".into(),
                downloaded_bytes: 0,
                total_bytes: None,
                status: models::DownloadStatus::Complete,
            })
            .map_err(|e| format!("Channel send: {e}"))?;
        return Ok(());
    }

    let channel_clone = channel.clone();
    std::thread::Builder::new()
        .name("model-download".into())
        .spawn(move || {
            let result = models::download::download_model(&model_dir, |progress| {
                let _ = channel_clone.send(progress);
            });
            match result {
                Ok(()) => {
                    let _ = channel_clone.send(models::DownloadProgress {
                        file: "all".into(),
                        downloaded_bytes: 0,
                        total_bytes: None,
                        status: models::DownloadStatus::Complete,
                    });
                }
                Err(e) => {
                    let _ = channel_clone.send(models::DownloadProgress {
                        file: "error".into(),
                        downloaded_bytes: 0,
                        total_bytes: None,
                        status: models::DownloadStatus::Error(e),
                    });
                }
            }
        })
        .map_err(|e| format!("Failed to spawn download thread: {e}"))?;

    Ok(())
}

/// Load the model into memory (GPU/CPU). Must be called after download.
/// Also spawns the persistent inference pipeline thread.
#[tauri::command]
pub fn load_model(state: State<'_, AppState>) -> Result<(), String> {
    let model_dir = models::default_model_dir();
    if !models::model_exists(&model_dir) {
        return Err("Model not downloaded yet".into());
    }

    let mut engine = state.engine.acquire()?;
    info!(path = %model_dir.display(), "Loading model");
    engine
        .load_model(&model_dir)
        .map_err(|e: crate::engine::EngineError| e.to_string())?;
    drop(engine);

    *state.model_loaded.acquire()? = true;

    // Spawn persistent inference pipeline (lives until app exit)
    let pipeline =
        TranscriptionPipeline::spawn(state.audio_receiver.clone(), Arc::clone(&state.engine))?;
    *state.pipeline.acquire()? = Some(pipeline);

    info!("Model loaded, inference pipeline ready");
    Ok(())
}

/// Debug: feed the debug WAV through the engine to test model in isolation.
#[tauri::command]
pub fn test_transcribe_wav(state: State<'_, AppState>) -> Result<String, String> {
    use crate::constants::{SAMPLE_RATE_F64, SILENCE_SUFFIX_SAMPLES};
    use crate::engine::TranscriptionEngine;

    let wav_path = crate::constants::app_data_dir().join("debug_engine_input.wav");

    if !wav_path.exists() {
        return Err("No debug WAV found. Record something first.".into());
    }

    let mut reader = hound::WavReader::open(&wav_path).map_err(|e| format!("WAV read: {e}"))?;
    let pcm: Vec<f32> = reader.samples::<f32>().filter_map(|s| s.ok()).collect();
    info!(
        samples = pcm.len(),
        duration_s = format_args!("{:.1}", pcm.len() as f64 / SAMPLE_RATE_F64),
        "Test WAV loaded"
    );

    // Add silence suffix for flush
    let mut audio = pcm;
    audio.resize(audio.len() + SILENCE_SUFFIX_SAMPLES, 0.0);

    let engine = state.engine.acquire()?;
    engine.reset_state().map_err(|e| e.to_string())?;

    let result = engine.transcribe(&audio, None).map_err(|e| e.to_string())?;

    let text: String = result
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    info!(segments = result.len(), text = %text, "Test transcription result");

    if text.is_empty() {
        Ok("No words detected (model produced 0 segments)".into())
    } else {
        Ok(text)
    }
}
