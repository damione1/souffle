use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter, State, ipc::Channel};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tracing::info;

use crate::audio::AudioChunk;
use crate::audio::capture::{AudioDeviceInfo, list_input_devices};
use crate::engine::TranscriptionEngine;
use crate::models;
use crate::pipeline::{InferenceCommand, SegmentCallback, TranscriptionPipeline};
use crate::state::{AppState, AudioCommand, MeetingAccumulator, RecordingMode};

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
    let loaded = *state
        .model_loaded
        .lock()
        .map_err(|e| format!("Lock: {e}"))?;
    let engine_name = state
        .engine
        .lock()
        .map(|e| e.name().to_string())
        .unwrap_or_else(|_| "Unknown".into());

    Ok(ModelStatus {
        downloaded,
        loaded,
        model_dir: model_dir.display().to_string(),
        engine_name,
    })
}

/// List available audio input devices
#[tauri::command]
pub fn list_audio_devices() -> Vec<AudioDeviceInfo> {
    list_input_devices()
}

/// Select an audio input device by name
#[tauri::command]
pub fn select_audio_device(state: State<'_, AppState>, device_name: String) -> Result<(), String> {
    state
        .audio_cmd_sender
        .send(AudioCommand::SelectDevice(device_name))
        .map_err(|e| format!("Failed to send device selection: {e}"))
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

    // Run download on a blocking thread (hf-hub is sync)
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

    let mut engine = state.engine.lock().map_err(|e| format!("Lock: {e}"))?;
    eprintln!("[souffle] Loading model from {}", model_dir.display());
    engine.load_model(&model_dir).map_err(|e| e.to_string())?;
    drop(engine);

    let mut loaded = state
        .model_loaded
        .lock()
        .map_err(|e| format!("Lock: {e}"))?;
    *loaded = true;

    // Spawn persistent inference pipeline (lives until app exit)
    let pipeline =
        TranscriptionPipeline::spawn(state.audio_receiver.clone(), Arc::clone(&state.engine));
    let mut pipe_guard = state.pipeline.lock().map_err(|e| format!("Lock: {e}"))?;
    *pipe_guard = Some(pipeline);

    eprintln!("[souffle] Model loaded, inference pipeline ready");
    Ok(())
}

fn next_audio_session_id(state: &AppState) -> Result<u64, String> {
    let mut guard = state
        .next_audio_session_id
        .lock()
        .map_err(|e| format!("Lock: {e}"))?;
    *guard += 1;
    Ok(*guard)
}

fn drain_audio_queue(receiver: &crossbeam_channel::Receiver<AudioChunk>) -> usize {
    let mut drained = 0usize;
    while receiver.try_recv().is_ok() {
        drained += 1;
    }
    drained
}

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

    let session_id = next_audio_session_id(&state)?;
    state
        .audio_cmd_sender
        .send(AudioCommand::Start(session_id))
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
        all_samples.extend_from_slice(&chunk.samples);
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
        all_samples.len() as f64 / 24_000.0,
        path_str
    ))
}

/// Start streaming transcription. Audio is captured and transcribed in real-time.
/// Segments are streamed back via the Channel API.
/// The pipeline thread is already running (spawned at model load) — we reset
/// the ASR state here (main thread) then send Start to the pipeline.
#[tauri::command]
pub fn start_transcription(
    state: State<'_, AppState>,
    channel: Channel<crate::engine::TranscriptionSegment>,
) -> Result<(), String> {
    let loaded = *state
        .model_loaded
        .lock()
        .map_err(|e| format!("Lock: {e}"))?;
    if !loaded {
        return Err("Model not loaded".into());
    }

    let mut is_recording = state
        .is_recording
        .lock()
        .map_err(|e| format!("Lock: {e}"))?;
    if *is_recording {
        return Err("Already recording".into());
    }

    eprintln!("[souffle] Starting streaming transcription...");
    let session_id = next_audio_session_id(&state)?;

    // Clear any tail chunks left behind by the previous session before reset.
    let drained = drain_audio_queue(&state.audio_receiver);
    if crate::debug::transcription_debug_enabled() && drained > 0 {
        eprintln!("[souffle] Cleared {drained} stale audio chunks before reset");
    }

    // Reset while the pipeline is idle so session teardown/rebuild is fully
    // serialized away from active inference.
    state
        .engine
        .lock()
        .map_err(|e| format!("Lock: {e}"))?
        .reset_state()
        .map_err(|e| format!("State reset: {e}"))?;

    // Send Start before audio capture begins so the next chunk processed belongs
    // to this session, not to the start/reset race.
    let channel_clone = channel.clone();
    let on_segment: SegmentCallback = Box::new(move |seg: crate::engine::TranscriptionSegment| {
        let _ = channel_clone.send(seg);
    });

    let pipe_guard = state.pipeline.lock().map_err(|e| format!("Lock: {e}"))?;
    let pipeline = pipe_guard.as_ref().ok_or("Pipeline not initialized")?;
    pipeline.send(InferenceCommand::Start {
        session_id,
        on_segment,
    })?;

    state
        .audio_cmd_sender
        .send(AudioCommand::Start(session_id))
        .map_err(|e| format!("Audio start: {e}"))?;

    *is_recording = true;
    let mut mode = state
        .recording_mode
        .lock()
        .map_err(|e| format!("Lock: {e}"))?;
    *mode = RecordingMode::Dictation;

    eprintln!("[souffle] Streaming transcription active");
    Ok(())
}

/// Stop streaming transcription.
/// Pipeline stays alive — it drains remaining audio and flushes the engine
/// before the next session can start.
#[tauri::command]
pub fn stop_transcription(state: State<'_, AppState>) -> Result<(), String> {
    let mut is_recording = state
        .is_recording
        .lock()
        .map_err(|e| format!("Lock: {e}"))?;
    if !*is_recording {
        return Err("Not recording".into());
    }

    // 1. Stop audio capture FIRST — no more new audio enters the channel
    state
        .audio_cmd_sender
        .send(AudioCommand::Stop)
        .map_err(|e| format!("Audio stop: {e}"))?;

    // 2. Wait for audio thread to flush its internal buffers to the channel
    std::thread::sleep(Duration::from_millis(300));

    // 3. Stop the pipeline session — it drains remaining audio and flushes engine,
    //    then signals completion via the done channel
    let (done_tx, done_rx) = crossbeam_channel::bounded(1);
    {
        let pipe_guard = state.pipeline.lock().map_err(|e| format!("Lock: {e}"))?;
        if let Some(pipeline) = pipe_guard.as_ref() {
            pipeline.send(InferenceCommand::Stop(done_tx))?;
        }
    }

    // 4. Wait for drain/flush to complete before allowing another session
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "Pipeline drain timeout".to_string())?;

    let drained = drain_audio_queue(&state.audio_receiver);
    if crate::debug::transcription_debug_enabled() && drained > 0 {
        eprintln!("[souffle] Cleared {drained} trailing audio chunks after stop");
    }

    *is_recording = false;
    let mut mode = state
        .recording_mode
        .lock()
        .map_err(|e| format!("Lock: {e}"))?;
    *mode = RecordingMode::Idle;

    info!("Streaming transcription stopped");
    Ok(())
}

/// Start meeting recording with live transcription.
/// Segments are accumulated for later storage.
#[tauri::command]
pub fn start_meeting_recording(
    state: State<'_, AppState>,
    title: String,
    channel: Channel<crate::engine::TranscriptionSegment>,
) -> Result<(), String> {
    let loaded = *state
        .model_loaded
        .lock()
        .map_err(|e| format!("Lock: {e}"))?;
    if !loaded {
        return Err("Model not loaded".into());
    }

    let mut is_recording = state
        .is_recording
        .lock()
        .map_err(|e| format!("Lock: {e}"))?;
    if *is_recording {
        return Err("Already recording".into());
    }

    eprintln!("[souffle] Starting meeting recording: {title}");
    let session_id = next_audio_session_id(&state)?;

    // Initialize meeting accumulator
    {
        let mut acc = state
            .meeting_accumulator
            .lock()
            .map_err(|e| format!("Lock: {e}"))?;
        *acc = Some(MeetingAccumulator {
            title,
            segments: Vec::new(),
            started_at: chrono::Utc::now(),
        });
    }

    let drained = drain_audio_queue(&state.audio_receiver);
    if crate::debug::transcription_debug_enabled() && drained > 0 {
        eprintln!("[souffle] Cleared {drained} stale audio chunks before reset");
    }

    // Reset while the pipeline is idle so session teardown/rebuild is fully
    // serialized away from active inference.
    state
        .engine
        .lock()
        .map_err(|e| format!("Lock: {e}"))?
        .reset_state()
        .map_err(|e| format!("State reset: {e}"))?;

    // Send Start before audio capture begins so the next chunk processed belongs
    // to this session, not to the start/reset race.
    let channel_clone = channel.clone();
    let acc_ref = Arc::clone(&state.meeting_accumulator);
    let on_segment: SegmentCallback = Box::new(move |seg: crate::engine::TranscriptionSegment| {
        let _ = channel_clone.send(seg.clone());
        if let Ok(mut acc) = acc_ref.lock() {
            if let Some(ref mut meeting) = *acc {
                meeting.segments.push(seg);
            }
        }
    });

    let pipe_guard = state.pipeline.lock().map_err(|e| format!("Lock: {e}"))?;
    let pipeline = pipe_guard.as_ref().ok_or("Pipeline not initialized")?;
    pipeline.send(InferenceCommand::Start {
        session_id,
        on_segment,
    })?;

    state
        .audio_cmd_sender
        .send(AudioCommand::Start(session_id))
        .map_err(|e| format!("Audio start: {e}"))?;

    *is_recording = true;
    let mut mode = state
        .recording_mode
        .lock()
        .map_err(|e| format!("Lock: {e}"))?;
    *mode = RecordingMode::Meeting;

    info!("Meeting recording started");
    Ok(())
}

/// Stop meeting recording and save transcript.
#[tauri::command]
pub fn stop_meeting_recording(state: State<'_, AppState>) -> Result<String, String> {
    let mut is_recording = state
        .is_recording
        .lock()
        .map_err(|e| format!("Lock: {e}"))?;
    if !*is_recording {
        return Err("Not recording".into());
    }

    // 1. Stop audio capture FIRST
    state
        .audio_cmd_sender
        .send(AudioCommand::Stop)
        .map_err(|e| format!("Audio stop: {e}"))?;

    // 2. Wait for audio thread to flush to channel
    std::thread::sleep(Duration::from_millis(300));

    // 3. Stop pipeline session — drains remaining audio and flushes engine
    let (done_tx, done_rx) = crossbeam_channel::bounded(1);
    {
        let pipe_guard = state.pipeline.lock().map_err(|e| format!("Lock: {e}"))?;
        if let Some(pipeline) = pipe_guard.as_ref() {
            pipeline.send(InferenceCommand::Stop(done_tx))?;
        }
    }
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "Pipeline drain timeout".to_string())?;

    let drained = drain_audio_queue(&state.audio_receiver);
    if crate::debug::transcription_debug_enabled() && drained > 0 {
        eprintln!("[souffle] Cleared {drained} trailing audio chunks after stop");
    }

    *is_recording = false;
    let mut mode = state
        .recording_mode
        .lock()
        .map_err(|e| format!("Lock: {e}"))?;
    *mode = RecordingMode::Idle;

    // Save meeting transcript
    let mut acc_guard = state
        .meeting_accumulator
        .lock()
        .map_err(|e| format!("Lock: {e}"))?;
    let meeting = acc_guard.take().ok_or("No meeting accumulator")?;

    let now = chrono::Utc::now();
    let duration = (now - meeting.started_at).num_seconds() as f64;
    let id = uuid::Uuid::new_v4().to_string();

    let engine_name = state
        .engine
        .lock()
        .map(|e| e.name().to_string())
        .unwrap_or_else(|_| "Unknown".into());

    let transcript = crate::transcript::MeetingTranscript {
        id: id.clone(),
        title: meeting.title,
        started_at: meeting.started_at,
        ended_at: Some(now),
        duration_seconds: duration,
        engine: engine_name,
        segments: meeting.segments,
        summary: None,
        summary_model: None,
        summary_generated_at: None,
    };

    state.db.save_meeting(&transcript)?;
    info!(id = %id, duration = duration, "Meeting recording saved");

    Ok(id)
}

/// List all saved meetings
#[tauri::command]
pub fn list_meetings(
    state: State<'_, AppState>,
) -> Result<Vec<crate::transcript::MeetingListItem>, String> {
    state.db.list_meetings()
}

/// Get a full meeting transcript by ID
#[tauri::command]
pub fn get_meeting(
    state: State<'_, AppState>,
    id: String,
) -> Result<crate::transcript::MeetingTranscript, String> {
    state.db.load_meeting(&id)
}

/// Delete a meeting by ID
#[tauri::command]
pub fn delete_meeting(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.db.delete_meeting(&id)
}

/// Copy text to clipboard and simulate Cmd+V paste
#[tauri::command]
pub fn paste_text(text: String, delay_ms: u64) -> Result<(), String> {
    crate::clipboard::copy_and_paste(&text, delay_ms)
}

/// Check if Ollama is available and list models
#[tauri::command]
pub async fn check_ollama() -> Result<crate::ollama::OllamaStatus, String> {
    Ok(crate::ollama::check_available(None).await)
}

/// Summarize a meeting transcript using Ollama, streaming results back
#[tauri::command]
pub async fn summarize_meeting(
    state: State<'_, AppState>,
    id: String,
    model: String,
    channel: Channel<crate::ollama::SummarizeProgress>,
) -> Result<(), String> {
    let transcript = state.db.load_meeting(&id)?;

    // Build transcript text from segments (space-separated — SentencePiece strips inter-word spaces)
    let text: String = transcript
        .segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    if text.is_empty() {
        return Err("Transcript has no text".into());
    }

    let channel_clone = channel.clone();
    let db = state.db.clone();
    let summary = crate::ollama::summarize_stream(&text, &model, None, move |progress| {
        let _ = channel_clone.send(progress);
    })
    .await?;

    // Save summary to database
    db.update_meeting_summary(&id, &summary, &model)?;

    Ok(())
}

/// Debug: feed the debug WAV through the engine to test model in isolation.
#[tauri::command]
pub fn test_transcribe_wav(state: State<'_, AppState>) -> Result<String, String> {
    let wav_path = dirs_next::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.souffle.app")
        .join("debug_engine_input.wav");

    if !wav_path.exists() {
        return Err("No debug WAV found. Record something first.".into());
    }

    // Read WAV
    let mut reader = hound::WavReader::open(&wav_path).map_err(|e| format!("WAV read: {e}"))?;
    let pcm: Vec<f32> = reader.samples::<f32>().filter_map(|s| s.ok()).collect();
    eprintln!(
        "[souffle] Test WAV: {} samples ({:.1}s)",
        pcm.len(),
        pcm.len() as f64 / 24000.0
    );

    // Add silence suffix for flush
    let mut audio = pcm;
    audio.resize(audio.len() + 36000, 0.0); // 1.5s silence suffix

    // Reset state and run
    let engine = state.engine.lock().map_err(|e| format!("Lock: {e}"))?;
    engine.reset_state().map_err(|e| e.to_string())?;

    let result = engine.transcribe(&audio, None).map_err(|e| e.to_string())?;

    let text: String = result
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    eprintln!(
        "[souffle] Test result: {} segments, text={:?}",
        result.len(),
        text
    );

    if text.is_empty() {
        Ok("No words detected (model produced 0 segments)".into())
    } else {
        Ok(text)
    }
}

// ─── Dictation history commands ──────────────────────────────────────

/// List dictation history entries
#[tauri::command]
pub fn list_dictation_entries(
    state: State<'_, AppState>,
    limit: Option<i64>,
) -> Result<Vec<crate::db::dictation::DictationEntry>, String> {
    state.db.list_dictation_entries(limit.unwrap_or(50))
}

/// Add a dictation history entry
#[tauri::command]
pub fn add_dictation_entry(state: State<'_, AppState>, text: String) -> Result<(), String> {
    let id = uuid::Uuid::new_v4().to_string();
    let timestamp = chrono::Utc::now().to_rfc3339();
    state.db.add_dictation_entry(&id, &text, &timestamp)
}

/// Delete a single dictation entry
#[tauri::command]
pub fn delete_dictation_entry(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.db.delete_dictation_entry(&id)
}

/// Clear all dictation history
#[tauri::command]
pub fn clear_dictation_history(state: State<'_, AppState>) -> Result<(), String> {
    state.db.clear_dictation_entries()
}

// ─── Settings commands ──────────────────────────────────────────────

/// Get all settings as a JSON object
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let pairs = state.db.get_all_settings()?;
    let mut map = serde_json::Map::new();
    for (key, value_str) in pairs {
        // Each value is stored as JSON, parse it back
        let value: serde_json::Value =
            serde_json::from_str(&value_str).unwrap_or(serde_json::Value::String(value_str));
        map.insert(key, value);
    }
    Ok(serde_json::Value::Object(map))
}

/// Save a single setting (key + JSON-encoded value)
#[tauri::command]
pub fn save_setting(
    state: State<'_, AppState>,
    key: String,
    value: serde_json::Value,
) -> Result<(), String> {
    if key == "debug_transcription" {
        if let Some(enabled) = value.as_bool() {
            crate::debug::set_transcription_debug(enabled);
        }
    }
    let value_str = serde_json::to_string(&value).map_err(|e| format!("Serialize: {e}"))?;
    state.db.set_setting(&key, &value_str)
}

// ─── Shortcut commands ──────────────────────────────────────────────

/// Register global shortcuts for toggle and push-to-talk dictation.
/// Called from setup() on startup and from update_shortcuts command.
pub fn register_shortcuts(
    app: &AppHandle,
    toggle_shortcut: &str,
    ptt_shortcut: &str,
) -> Result<(), String> {
    let gs = app.global_shortcut();

    // Unregister all existing shortcuts first
    gs.unregister_all()
        .map_err(|e| format!("Unregister: {e}"))?;

    // Toggle shortcut: emit event, frontend handles the pipeline
    if !toggle_shortcut.is_empty() {
        gs.on_shortcut(toggle_shortcut, move |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                let _ = app.emit("shortcut-toggle", ());
            }
        })
        .map_err(|e| format!("Register toggle shortcut '{toggle_shortcut}': {e}"))?;
        info!(shortcut = toggle_shortcut, "Toggle shortcut registered");
    }

    // Push-to-talk shortcut: emit start on press, stop on release
    if !ptt_shortcut.is_empty() {
        gs.on_shortcut(ptt_shortcut, move |app, _shortcut, event| {
            match event.state {
                ShortcutState::Pressed => {
                    let _ = app.emit("shortcut-ptt-start", ());
                }
                ShortcutState::Released => {
                    let _ = app.emit("shortcut-ptt-stop", ());
                }
            }
        })
        .map_err(|e| format!("Register PTT shortcut '{ptt_shortcut}': {e}"))?;
        info!(shortcut = ptt_shortcut, "Push-to-talk shortcut registered");
    }

    Ok(())
}

/// Update shortcut bindings at runtime. Saves to DB and re-registers.
#[tauri::command]
pub fn update_shortcuts(
    app: AppHandle,
    state: State<'_, AppState>,
    toggle_shortcut: String,
    ptt_shortcut: String,
) -> Result<(), String> {
    // Save to database
    let toggle_json =
        serde_json::to_string(&toggle_shortcut).map_err(|e| format!("Serialize: {e}"))?;
    let ptt_json = serde_json::to_string(&ptt_shortcut).map_err(|e| format!("Serialize: {e}"))?;

    state.db.set_setting("shortcut_toggle", &toggle_json)?;
    state.db.set_setting("shortcut_push_to_talk", &ptt_json)?;

    // Re-register shortcuts
    register_shortcuts(&app, &toggle_shortcut, &ptt_shortcut)
}

/// Get current shortcut settings
#[tauri::command]
pub fn get_shortcuts(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let toggle = state
        .db
        .get_setting("shortcut_toggle")?
        .and_then(|v| serde_json::from_str::<String>(&v).ok())
        .unwrap_or_else(|| crate::DEFAULT_TOGGLE_SHORTCUT.to_string());

    let ptt = state
        .db
        .get_setting("shortcut_push_to_talk")?
        .and_then(|v| serde_json::from_str::<String>(&v).ok())
        .unwrap_or_default();

    Ok(serde_json::json!({
        "toggle": toggle,
        "push_to_talk": ptt,
    }))
}

/// Save f32 PCM samples (24kHz mono) to a WAV file
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
        sample_rate: 24_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let mut writer =
        hound::WavWriter::create(&path, spec).map_err(|e| format!("Failed to create WAV: {e}"))?;

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
