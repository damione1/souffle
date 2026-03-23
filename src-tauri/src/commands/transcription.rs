use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tauri::State;
use tauri::ipc::Channel;
use tracing::info;

use crate::audio::AudioChunk;
use crate::constants::{AUDIO_FLUSH_MS, PIPELINE_DRAIN_TIMEOUT_SECS, SAMPLE_RATE_F64};
use crate::engine::TranscriptionEngine;
use crate::lock_ext::MutexExt;
use crate::pipeline::{InferenceCommand, SegmentCallback};
use crate::state::{AppState, AudioCommand, MeetingAccumulator, RecordingMode};

fn next_audio_session_id(state: &AppState) -> Result<u64, String> {
    let mut guard = state.next_audio_session_id.acquire()?;
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

/// Shared logic for starting a recording pipeline.
/// Validates preconditions, drains stale audio, resets engine, sends Start command, begins audio capture.
fn start_pipeline(
    state: &AppState,
    on_segment: SegmentCallback,
) -> Result<u64, String> {
    let loaded = *state.model_loaded.acquire()?;
    if !loaded {
        return Err("Model not loaded".into());
    }

    let mut is_recording = state.is_recording.acquire()?;
    if *is_recording {
        return Err("Already recording".into());
    }

    let session_id = next_audio_session_id(state)?;

    // Clear stale audio chunks before reset
    let drained = drain_audio_queue(&state.audio_receiver);
    if crate::debug::transcription_debug_enabled() && drained > 0 {
        tracing::debug!(drained, "Cleared stale audio chunks before reset");
    }

    // Reset while the pipeline is idle
    state
        .engine
        .acquire()?
        .reset_state()
        .map_err(|e| format!("State reset: {e}"))?;

    // Send Start before audio capture begins
    let pipe_guard = state.pipeline.acquire()?;
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
    Ok(session_id)
}

/// Shared logic for stopping a recording pipeline.
/// Stops audio, flushes buffers, drains pipeline, sets idle state.
fn stop_pipeline(state: &AppState) -> Result<(), String> {
    // 1. Stop audio capture FIRST
    state
        .audio_cmd_sender
        .send(AudioCommand::Stop)
        .map_err(|e| format!("Audio stop: {e}"))?;

    // 2. Wait for audio thread to flush its internal buffers
    std::thread::sleep(Duration::from_millis(AUDIO_FLUSH_MS));

    // 3. Stop pipeline session — drains remaining audio and flushes engine
    let (done_tx, done_rx) = crossbeam_channel::bounded(1);
    {
        let pipe_guard = state.pipeline.acquire()?;
        if let Some(pipeline) = pipe_guard.as_ref() {
            pipeline.send(InferenceCommand::Stop(done_tx))?;
        }
    }

    // 4. Wait for drain/flush to complete
    done_rx
        .recv_timeout(Duration::from_secs(PIPELINE_DRAIN_TIMEOUT_SECS))
        .map_err(|_| "Pipeline drain timeout".to_string())?;

    let drained = drain_audio_queue(&state.audio_receiver);
    if crate::debug::transcription_debug_enabled() && drained > 0 {
        tracing::debug!(drained, "Cleared trailing audio chunks after stop");
    }

    let mut is_recording = state.is_recording.acquire()?;
    *is_recording = false;
    *state.recording_mode.acquire()? = RecordingMode::Idle;

    Ok(())
}

/// Start recording audio from the default microphone
#[tauri::command]
pub fn start_recording(state: State<'_, AppState>) -> Result<(), String> {
    let mut is_recording = state.is_recording.acquire()?;

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
    let mut is_recording = state.is_recording.acquire()?;

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
        all_samples.len() as f64 / SAMPLE_RATE_F64,
        path_str
    ))
}

/// Start streaming transcription.
#[tauri::command]
pub fn start_transcription(
    state: State<'_, AppState>,
    channel: Channel<crate::engine::TranscriptionSegment>,
) -> Result<(), String> {
    info!("Starting streaming transcription");

    let channel_clone = channel.clone();
    let on_segment: SegmentCallback = Box::new(move |seg| {
        let _ = channel_clone.send(seg);
    });

    start_pipeline(&state, on_segment)?;
    *state.recording_mode.acquire()? = RecordingMode::Dictation;

    info!("Streaming transcription active");
    Ok(())
}

/// Stop streaming transcription.
#[tauri::command]
pub fn stop_transcription(state: State<'_, AppState>) -> Result<(), String> {
    let is_recording = *state.is_recording.acquire()?;
    if !is_recording {
        return Err("Not recording".into());
    }

    stop_pipeline(&state)?;
    info!("Streaming transcription stopped");
    Ok(())
}

/// Start meeting recording with live transcription.
#[tauri::command]
pub fn start_meeting_recording(
    state: State<'_, AppState>,
    title: String,
    channel: Channel<crate::engine::TranscriptionSegment>,
) -> Result<(), String> {
    info!(title = %title, "Starting meeting recording");

    // Initialize meeting accumulator
    {
        let mut acc = state.meeting_accumulator.acquire()?;
        *acc = Some(MeetingAccumulator {
            title,
            segments: Vec::new(),
            started_at: chrono::Utc::now(),
        });
    }

    let channel_clone = channel.clone();
    let acc_ref = Arc::clone(&state.meeting_accumulator);
    let on_segment: SegmentCallback = Box::new(move |seg| {
        let _ = channel_clone.send(seg.clone());
        if let Ok(mut acc) = acc_ref.lock() {
            if let Some(ref mut meeting) = *acc {
                meeting.segments.push(seg);
            }
        }
    });

    start_pipeline(&state, on_segment)?;
    *state.recording_mode.acquire()? = RecordingMode::Meeting;

    info!("Meeting recording started");
    Ok(())
}

/// Stop meeting recording and save transcript.
#[tauri::command]
pub fn stop_meeting_recording(state: State<'_, AppState>) -> Result<String, String> {
    let is_recording = *state.is_recording.acquire()?;
    if !is_recording {
        return Err("Not recording".into());
    }

    stop_pipeline(&state)?;

    // Save meeting transcript
    let mut acc_guard = state.meeting_accumulator.acquire()?;
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

/// Copy text to clipboard and simulate Cmd+V paste
#[tauri::command]
pub fn paste_text(text: String, delay_ms: u64) -> Result<(), String> {
    crate::clipboard::copy_and_paste(&text, delay_ms)
}

/// Save f32 PCM samples (24kHz mono) to a WAV file
fn save_wav(samples: &[f32]) -> Result<PathBuf, String> {
    use crate::constants::SAMPLE_RATE;

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
        sample_rate: SAMPLE_RATE,
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
