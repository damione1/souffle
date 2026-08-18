use tauri::Manager;
use tauri::State;
use tauri::ipc::Channel;
use tracing::info;

use crate::engine::{
    TranscriptionCatalog, TranscriptionProfile, TranscriptionProfileSelection,
    TranscriptionRuntimeStatus, resolve_transcription_profile, resolve_transcription_selection,
    transcription_engine_catalog,
};
use crate::models;
use crate::settings::AppSettings;
use crate::state::AppState;
use crate::state_machine::{AppStateMachine, StateAction};

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

    // Derive phase from the state machine
    let machine = state.current_machine_state()?;
    let phase = if machine.is_model_ready() && machine.active_profile() == Some(&profile) {
        crate::engine::TranscriptionRuntimePhase::Ready
    } else if models::model_exists(&profile) {
        crate::engine::TranscriptionRuntimePhase::LoadRequired
    } else {
        crate::engine::TranscriptionRuntimePhase::DownloadRequired
    };

    Ok(TranscriptionRuntimeStatus {
        profile: profile.clone(),
        phase,
        model_dir: model_dir.display().to_string(),
    })
}

/// Return the current state machine state.
#[tauri::command]
#[specta::specta]
pub fn get_machine_state(state: State<'_, AppState>) -> Result<AppStateMachine, String> {
    state.current_machine_state()
}

/// Recover from an error state.
#[tauri::command]
#[specta::specta]
pub fn recover_state(state: State<'_, AppState>) -> Result<AppStateMachine, String> {
    state.apply_transition(StateAction::Recover)
}

/// Download the selected transcription model.
/// Progress is streamed back via the Channel API.
#[tauri::command]
#[specta::specta]
pub fn download_model(
    state: State<'_, AppState>,
    selection: TranscriptionProfileSelection,
    channel: Channel<models::DownloadProgress>,
) -> Result<(), String> {
    let profile = resolve_transcription_selection(&selection)?;
    if models::model_exists(&profile) {
        // Ensure machine reflects downloaded state
        let machine = state.current_machine_state()?;
        if matches!(machine, AppStateMachine::Idle) {
            let _ = state.apply_transition(StateAction::StartDownload {
                profile: profile.clone(),
            });
            let _ = state.apply_transition(StateAction::DownloadComplete);
        }

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

    // Switching to a different, not-yet-downloaded model while one is loaded:
    // drop the current engine first so the state machine can move out of
    // Ready (it tracks a single profile through download → load → ready).
    let machine = state.current_machine_state()?;
    if machine.is_recording() {
        return Err("Cannot switch models while recording".into());
    }
    if machine.is_model_ready() && machine.active_profile() != Some(&profile) {
        state.apply_transition(StateAction::Unload { next_profile: None })?;
        state.engine_actor.unload_model()?;
        state.apply_transition(StateAction::UnloadComplete)?;
        // Machine is now Downloaded { current } — different from `profile`.
    }

    // Transition to Downloading
    state.apply_transition(StateAction::StartDownload {
        profile: profile.clone(),
    })?;

    // Clone what we need for the thread
    let channel_clone = channel.clone();
    let app_handle_for_state: Option<tauri::AppHandle> =
        state.app_handle.lock().ok().and_then(|guard| guard.clone());

    std::thread::Builder::new()
        .name("model-download".into())
        .spawn(move || {
            let result = models::download_model(&profile, |progress| {
                let _ = channel_clone.send(progress);
            });

            // Helper to apply transition from the thread via AppHandle
            let apply = |action: StateAction| {
                if let Some(ref handle) = app_handle_for_state {
                    let state: tauri::State<'_, AppState> = handle.state();
                    let _ = state.apply_transition(action);
                }
            };

            match result {
                Ok(()) => {
                    apply(StateAction::DownloadComplete);
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
                    apply(StateAction::Fail { message: e.clone() });
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
/// The engine actor creates the engine, swaps out any previous one, and
/// loads the weights — all on its own thread.
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

    let machine = state.current_machine_state()?;
    if machine.is_recording() {
        return Err("Cannot load the model while recording".into());
    }

    // Auto-recover from Error state so "Load model" works after a failure
    if matches!(machine, AppStateMachine::Error { .. }) {
        state.apply_transition(StateAction::Recover)?;
    }

    let machine = state.current_machine_state()?;

    // Check if already loaded with this profile
    if machine.is_model_ready() && machine.active_profile() == Some(&profile) {
        info!("Model already loaded, reusing existing engine");
        return Ok(());
    }

    // If model is ready with a different profile, unload first
    if machine.is_model_ready() && machine.active_profile() != Some(&profile) {
        state.apply_transition(StateAction::Unload {
            next_profile: Some(profile.clone()),
        })?;
        state.engine_actor.unload_model()?;
        state.apply_transition(StateAction::UnloadComplete)?;
        // Machine is now in Loading { next_profile }
    } else {
        // Ensure machine is in Downloaded state before loading
        let machine = state.current_machine_state()?;
        if matches!(machine, AppStateMachine::Idle) {
            // Model exists on disk but machine doesn't know — fix up
            state.apply_transition(StateAction::StartDownload {
                profile: profile.clone(),
            })?;
            state.apply_transition(StateAction::DownloadComplete)?;
        }

        state.apply_transition(StateAction::StartLoad)?;
    }

    info!(path = %model_dir.display(), "Loading model");
    if let Err(e) = state.engine_actor.load_model(profile, model_dir) {
        state.apply_transition(StateAction::Fail { message: e.clone() })?;
        return Err(e);
    }

    state.apply_transition(StateAction::LoadComplete)?;

    info!("Model loaded, engine ready");
    Ok(())
}

/// Delete a downloaded model from disk.
#[tauri::command]
#[specta::specta]
pub fn delete_model(
    state: State<'_, AppState>,
    selection: TranscriptionProfileSelection,
) -> Result<(), String> {
    let profile = resolve_transcription_selection(&selection)?;

    // Cannot delete the actively loaded model
    let machine = state.current_machine_state()?;
    if machine.is_model_ready() && machine.active_profile() == Some(&profile) {
        return Err("Cannot delete the currently loaded model. Unload it first or switch to a different model.".into());
    }

    let model_dir = models::model_dir(&profile);
    models::download::delete_model_files(&model_dir)?;

    info!(
        engine = %profile.engine_id,
        model = %profile.model_id,
        "Model files deleted"
    );

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

    let result = state.engine_actor.debug_transcribe(audio)?;

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
