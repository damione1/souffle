use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tauri::{ipc::Channel, State};
use tracing::info;

use crate::audio::capture::{list_input_devices, AudioDeviceInfo};
use crate::engine::TranscriptionEngine;
use crate::models;
use crate::pipeline::{InferenceCommand, TranscriptionPipeline};
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
    let loaded = *state.model_loaded.lock().map_err(|e| format!("Lock: {e}"))?;
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
#[tauri::command]
pub fn load_model(state: State<'_, AppState>) -> Result<(), String> {
    let model_dir = models::default_model_dir();
    if !models::model_exists(&model_dir) {
        return Err("Model not downloaded yet".into());
    }

    let mut engine = state.engine.lock().map_err(|e| format!("Lock: {e}"))?;
    eprintln!("[souffle] Loading model from {}", model_dir.display());
    engine.load_model(&model_dir).map_err(|e| e.to_string())?;

    let mut loaded = state.model_loaded.lock().map_err(|e| format!("Lock: {e}"))?;
    *loaded = true;

    eprintln!("[souffle] Model loaded successfully");
    Ok(())
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
        all_samples.len() as f64 / 24_000.0,
        path_str
    ))
}

/// Start streaming transcription. Audio is captured and transcribed in real-time.
/// Segments are streamed back via the Channel API.
#[tauri::command]
pub fn start_transcription(
    state: State<'_, AppState>,
    channel: Channel<crate::engine::TranscriptionSegment>,
) -> Result<(), String> {
    let loaded = *state.model_loaded.lock().map_err(|e| format!("Lock: {e}"))?;
    if !loaded {
        return Err("Model not loaded".into());
    }

    let mut is_recording = state.is_recording.lock().map_err(|e| format!("Lock: {e}"))?;
    if *is_recording {
        return Err("Already recording".into());
    }

    eprintln!("[souffle] Starting streaming transcription...");

    // Reset ASR state for fresh session (clears KV cache from previous recording)
    state
        .engine
        .lock()
        .map_err(|e| format!("Lock: {e}"))?
        .reset_state()
        .map_err(|e| format!("State reset: {e}"))?;

    // Start audio capture
    state
        .audio_cmd_sender
        .send(AudioCommand::Start)
        .map_err(|e| format!("Audio start: {e}"))?;
    eprintln!("[souffle] Audio capture started");

    // Create pipeline with segment callback that sends to the Tauri channel
    let channel_clone = channel.clone();
    let on_segment = Box::new(move |seg: crate::engine::TranscriptionSegment| {
        let _ = channel_clone.send(seg);
    });

    let pipeline = TranscriptionPipeline::spawn(
        state.audio_receiver.clone(),
        Arc::clone(&state.engine),
        on_segment,
    );

    pipeline.send(InferenceCommand::Start)?;
    eprintln!("[souffle] Inference pipeline started");

    let mut pipe_guard = state.pipeline.lock().map_err(|e| format!("Lock: {e}"))?;
    *pipe_guard = Some(pipeline);

    *is_recording = true;
    let mut mode = state.recording_mode.lock().map_err(|e| format!("Lock: {e}"))?;
    *mode = RecordingMode::Dictation;

    eprintln!("[souffle] Streaming transcription active");
    Ok(())
}

/// Stop streaming transcription.
#[tauri::command]
pub fn stop_transcription(state: State<'_, AppState>) -> Result<(), String> {
    let mut is_recording = state.is_recording.lock().map_err(|e| format!("Lock: {e}"))?;
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

    // 3. Now stop the pipeline — it will drain remaining audio from channel and flush engine
    let mut pipe_guard = state.pipeline.lock().map_err(|e| format!("Lock: {e}"))?;
    if let Some(pipeline) = pipe_guard.as_ref() {
        pipeline.send(InferenceCommand::Stop)?;
    }
    // Give pipeline time to process remaining audio and flush
    std::thread::sleep(Duration::from_millis(500));
    *pipe_guard = None;

    *is_recording = false;
    let mut mode = state.recording_mode.lock().map_err(|e| format!("Lock: {e}"))?;
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
    let loaded = *state.model_loaded.lock().map_err(|e| format!("Lock: {e}"))?;
    if !loaded {
        return Err("Model not loaded".into());
    }

    let mut is_recording = state.is_recording.lock().map_err(|e| format!("Lock: {e}"))?;
    if *is_recording {
        return Err("Already recording".into());
    }

    eprintln!("[souffle] Starting meeting recording: {title}");

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

    // Reset ASR state
    state
        .engine
        .lock()
        .map_err(|e| format!("Lock: {e}"))?
        .reset_state()
        .map_err(|e| format!("State reset: {e}"))?;

    // Start audio capture
    state
        .audio_cmd_sender
        .send(AudioCommand::Start)
        .map_err(|e| format!("Audio start: {e}"))?;

    // Create pipeline — accumulate segments AND send to frontend channel
    let channel_clone = channel.clone();
    let acc_ref = Arc::clone(&state.meeting_accumulator);
    let on_segment = Box::new(move |seg: crate::engine::TranscriptionSegment| {
        // Send to frontend for live display
        let _ = channel_clone.send(seg.clone());
        // Accumulate for storage
        if let Ok(mut acc) = acc_ref.lock() {
            if let Some(ref mut meeting) = *acc {
                meeting.segments.push(seg);
            }
        }
    });

    let pipeline = TranscriptionPipeline::spawn(
        state.audio_receiver.clone(),
        Arc::clone(&state.engine),
        on_segment,
    );
    pipeline.send(InferenceCommand::Start)?;

    let mut pipe_guard = state.pipeline.lock().map_err(|e| format!("Lock: {e}"))?;
    *pipe_guard = Some(pipeline);

    *is_recording = true;
    let mut mode = state.recording_mode.lock().map_err(|e| format!("Lock: {e}"))?;
    *mode = RecordingMode::Meeting;

    info!("Meeting recording started");
    Ok(())
}

/// Stop meeting recording and save transcript to JSON.
#[tauri::command]
pub fn stop_meeting_recording(state: State<'_, AppState>) -> Result<String, String> {
    let mut is_recording = state.is_recording.lock().map_err(|e| format!("Lock: {e}"))?;
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

    // 3. Stop pipeline — drains remaining audio and flushes engine
    let mut pipe_guard = state.pipeline.lock().map_err(|e| format!("Lock: {e}"))?;
    if let Some(pipeline) = pipe_guard.as_ref() {
        pipeline.send(InferenceCommand::Stop)?;
    }
    std::thread::sleep(Duration::from_millis(500));
    *pipe_guard = None;

    *is_recording = false;
    let mut mode = state.recording_mode.lock().map_err(|e| format!("Lock: {e}"))?;
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

    crate::transcript::save_meeting(&transcript)?;
    info!(id = %id, duration = duration, "Meeting recording saved");

    Ok(id)
}

/// List all saved meetings
#[tauri::command]
pub fn list_meetings() -> Result<Vec<crate::transcript::MeetingListItem>, String> {
    crate::transcript::list_meetings()
}

/// Get a full meeting transcript by ID
#[tauri::command]
pub fn get_meeting(id: String) -> Result<crate::transcript::MeetingTranscript, String> {
    crate::transcript::load_meeting(&id)
}

/// Delete a meeting by ID
#[tauri::command]
pub fn delete_meeting(id: String) -> Result<(), String> {
    crate::transcript::delete_meeting(&id)
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
    id: String,
    model: String,
    channel: Channel<crate::ollama::SummarizeProgress>,
) -> Result<(), String> {
    let transcript = crate::transcript::load_meeting(&id)?;

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
    let summary = crate::ollama::summarize_stream(&text, &model, None, move |progress| {
        let _ = channel_clone.send(progress);
    })
    .await?;

    // Save summary to transcript
    crate::transcript::update_meeting_summary(&id, &summary, &model)?;

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
    let mut reader =
        hound::WavReader::open(&wav_path).map_err(|e| format!("WAV read: {e}"))?;
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
