use tauri::State;
use tauri::ipc::Channel;
use tracing::info;

use crate::engine::{
    TranscriptionCatalog, TranscriptionProfile, TranscriptionProfileSelection,
    TranscriptionRuntimeStatus, create_engine, resolve_transcription_profile,
    resolve_transcription_selection, transcription_engine_catalog,
    transcription_runtime_phase,
};
use crate::lock_ext::MutexExt;
use crate::models;
use crate::pipeline::TranscriptionPipeline;
use crate::settings::AppSettings;
use crate::state::AppState;
use std::sync::Arc;

fn selected_profile(state: &AppState) -> Result<TranscriptionProfile, String> {
    let settings = AppSettings::load(&state.db)?;
    resolve_transcription_profile(
        Some(&settings.transcription_engine_id),
        Some(&settings.transcription_model_id),
        Some(&settings.transcription_backend_id),
    )
}

/// Catalog of supported transcription engines and models.
#[tauri::command]
#[specta::specta]
pub fn get_transcription_catalog(
    state: State<'_, AppState>,
) -> Result<TranscriptionCatalog, String> {
    let profile = selected_profile(&state)?;
    Ok(TranscriptionCatalog {
        engines: transcription_engine_catalog(),
        selected_engine_id: profile.engine_id,
        selected_model_id: profile.model_id,
        selected_backend_id: profile.backend_id,
    })
}

/// Check whether the selected transcription model is downloaded and loaded.
#[tauri::command]
#[specta::specta]
pub fn get_model_status(
    state: State<'_, AppState>,
    selection: TranscriptionProfileSelection,
) -> Result<TranscriptionRuntimeStatus, String> {
    let profile = resolve_transcription_selection(&selection)?;
    let model_dir = models::model_dir(&profile);
    let downloaded = models::model_exists(&profile);
    let loaded = *state.model_loaded.acquire()?;
    let active_profile = state.active_profile.acquire()?.clone();
    let phase = transcription_runtime_phase(
        downloaded,
        loaded && active_profile.as_ref() == Some(&profile),
    );

    Ok(TranscriptionRuntimeStatus {
        profile: profile.clone(),
        phase,
        model_dir: model_dir.display().to_string(),
    })
}

/// Download the selected transcription model.
/// Progress is streamed back via the Channel API.
#[tauri::command]
#[specta::specta]
pub fn download_model(
    _state: State<'_, AppState>,
    selection: TranscriptionProfileSelection,
    channel: Channel<models::DownloadProgress>,
) -> Result<(), String> {
    let profile = resolve_transcription_selection(&selection)?;
    if models::model_exists(&profile) {
        channel
            .send(models::DownloadProgress {
                file: "all".into(),
                downloaded_bytes: 0,
                total_bytes: None,
                completed_files: 1,
                total_files: 1,
                status: models::DownloadStatus::Complete,
            })
            .map_err(|e| format!("Channel send: {e}"))?;
        return Ok(());
    }

    let channel_clone = channel.clone();
    std::thread::Builder::new()
        .name("model-download".into())
        .spawn(move || {
            let result = models::download_model(&profile, |progress| {
                let _ = channel_clone.send(progress);
            });
            match result {
                Ok(()) => {
                    let _ = channel_clone.send(models::DownloadProgress {
                        file: "all".into(),
                        downloaded_bytes: 0,
                        total_bytes: None,
                        completed_files: 1,
                        total_files: 1,
                        status: models::DownloadStatus::Complete,
                    });
                }
                Err(e) => {
                    let _ = channel_clone.send(models::DownloadProgress {
                        file: "error".into(),
                        downloaded_bytes: 0,
                        total_bytes: None,
                        completed_files: 0,
                        total_files: 1,
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
#[specta::specta]
pub fn load_model(
    state: State<'_, AppState>,
    selection: TranscriptionProfileSelection,
) -> Result<(), String> {
    let profile = resolve_transcription_selection(&selection)?;
    let model_dir = models::model_dir(&profile);
    if !models::model_exists(&profile) {
        return Err("Model not downloaded yet".into());
    }

    if *state.is_recording.acquire()? {
        return Err("Cannot load the model while recording".into());
    }

    {
        let model_loaded = *state.model_loaded.acquire()?;
        let active_profile = state.active_profile.acquire()?.clone();
        let pipeline_ready = state.pipeline.acquire()?.is_some();
        if model_loaded && pipeline_ready && active_profile.as_ref() == Some(&profile) {
            info!("Model already loaded, reusing existing inference pipeline");
            return Ok(());
        }
    }

    {
        let mut pipeline_guard = state.pipeline.acquire()?;
        if let Some(mut pipeline) = pipeline_guard.take() {
            pipeline.shutdown()?;
        }
    }
    *state.model_loaded.acquire()? = false;

    let current_profile = state.active_profile.acquire()?.clone();
    let mut engine = state.engine.acquire()?;
    if current_profile
        .as_ref()
        .is_none_or(|current| {
            current.engine_id != profile.engine_id || current.backend_id != profile.backend_id
        })
    {
        *engine = create_engine(&profile)?;
    }
    info!(path = %model_dir.display(), "Loading model");
    engine
        .load_model(&model_dir)
        .map_err(|e: crate::engine::EngineError| e.to_string())?;
    drop(engine);

    // Spawn persistent inference pipeline (lives until app exit)
    let pipeline =
        match TranscriptionPipeline::spawn(state.audio_receiver.clone(), Arc::clone(&state.engine))
        {
            Ok(pipeline) => pipeline,
            Err(e) => {
                let mut engine = state.engine.acquire()?;
                if let Err(unload_err) = engine.unload_model() {
                    tracing::warn!(
                        "Failed to unload model after pipeline startup error: {unload_err}"
                    );
                }
                return Err(e);
            }
        };

    *state.pipeline.acquire()? = Some(pipeline);
    *state.active_profile.acquire()? = Some(profile);
    *state.model_loaded.acquire()? = true;

    info!("Model loaded, inference pipeline ready");
    Ok(())
}

/// Debug: feed the debug WAV through the engine to test model in isolation.
#[tauri::command]
#[specta::specta]
pub fn test_transcribe_wav(state: State<'_, AppState>) -> Result<String, String> {
    use crate::constants::{SAMPLE_RATE_F64, SILENCE_SUFFIX_SAMPLES};

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
