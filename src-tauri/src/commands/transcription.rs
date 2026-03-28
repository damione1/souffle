use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tauri::State;
use tauri::ipc::Channel;
use tracing::info;
use uuid::Uuid;

use crate::audio::AudioChunk;
use crate::constants::{AUDIO_FLUSH_MS, PIPELINE_DRAIN_TIMEOUT_SECS, SAMPLE_RATE_F64};
use crate::lock_ext::MutexExt;
use crate::pipeline::{InferenceCommand, SegmentCallback};
use crate::state::{AppState, AudioCommand, MeetingAccumulator, RecordingMode};
use crate::transcript::{MeetingRecordingSession, MeetingTranscript};

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

fn current_active_profile(state: &AppState) -> Result<crate::engine::TranscriptionProfile, String> {
    state
        .active_profile
        .acquire()?
        .clone()
        .ok_or("No active transcription profile".to_string())
}

fn start_meeting_session(
    state: &AppState,
    accumulator: MeetingAccumulator,
    channel: Channel<crate::engine::TranscriptionSegment>,
) -> Result<(), String> {
    {
        let mut acc = state.meeting_accumulator.acquire()?;
        *acc = Some(accumulator);
    }

    let channel_clone = channel.clone();
    let acc_ref = Arc::clone(&state.meeting_accumulator);
    let on_segment: SegmentCallback = Box::new(move |seg| {
        let _ = channel_clone.send(seg.clone());
        if let Ok(mut acc) = acc_ref.lock()
            && let Some(ref mut meeting) = *acc
        {
            meeting.new_segments.push(seg);
        }
    });

    if let Err(error) = start_pipeline(state, on_segment) {
        let mut acc = state.meeting_accumulator.acquire()?;
        *acc = None;
        return Err(error);
    }

    *state.recording_mode.acquire()? = RecordingMode::Meeting;
    Ok(())
}

fn build_meeting_transcript(
    meeting: MeetingAccumulator,
    ended_at: chrono::DateTime<chrono::Utc>,
) -> MeetingTranscript {
    let mut segments = meeting.existing_segments;
    let start_segment_index = segments.len() as u64;
    let had_new_segments = !meeting.new_segments.is_empty();
    let has_summary = meeting.summary.is_some();
    segments.extend(meeting.new_segments);
    let end_segment_index = segments.len() as u64;

    let mut recording_sessions = meeting.recording_sessions;
    recording_sessions.push(MeetingRecordingSession::completed(
        Uuid::new_v4().to_string(),
        meeting.session_started_at,
        ended_at,
        start_segment_index,
        end_segment_index,
    ));

    let started_at = recording_sessions
        .first()
        .map(|session| session.started_at)
        .unwrap_or(meeting.session_started_at);
    let ended_at = recording_sessions.last().map(|session| session.ended_at);
    let duration_seconds = recording_sessions
        .iter()
        .map(|session| session.duration_seconds)
        .sum();

    MeetingTranscript {
        id: meeting.id,
        title: meeting.title,
        started_at,
        ended_at,
        duration_seconds,
        transcription_profile: meeting.transcription_profile,
        recording_sessions,
        segments,
        summary: meeting.summary,
        summary_is_stale: meeting.summary_is_stale || (has_summary && had_new_segments),
        summary_model: meeting.summary_model,
        summary_generated_at: meeting.summary_generated_at,
    }
}

/// Shared logic for starting a recording pipeline.
/// Validates preconditions, drains stale audio, resets engine, sends Start command, begins audio capture.
fn start_pipeline(state: &AppState, on_segment: SegmentCallback) -> Result<u64, String> {
    let loaded = *state.model_loaded.acquire()?;
    if !loaded {
        return Err("Model not loaded".into());
    }
    if state.active_profile.acquire()?.is_none() {
        return Err("No active transcription profile".into());
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
#[specta::specta]
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
#[specta::specta]
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
#[specta::specta]
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
#[specta::specta]
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
#[specta::specta]
pub fn start_meeting_recording(
    state: State<'_, AppState>,
    title: String,
    channel: Channel<crate::engine::TranscriptionSegment>,
) -> Result<(), String> {
    info!(title = %title, "Starting meeting recording");

    let accumulator = MeetingAccumulator {
        id: Uuid::new_v4().to_string(),
        title,
        existing_segments: Vec::new(),
        new_segments: Vec::new(),
        recording_sessions: Vec::new(),
        session_started_at: chrono::Utc::now(),
        transcription_profile: current_active_profile(&state)?,
        summary: None,
        summary_is_stale: false,
        summary_model: None,
        summary_generated_at: None,
    };

    start_meeting_session(&state, accumulator, channel)?;

    info!("Meeting recording started");
    Ok(())
}

/// Resume recording on an existing meeting and append new transcript segments.
#[tauri::command]
#[specta::specta]
pub fn resume_meeting_recording(
    state: State<'_, AppState>,
    meeting_id: String,
    channel: Channel<crate::engine::TranscriptionSegment>,
) -> Result<(), String> {
    info!(meeting_id = %meeting_id, "Resuming meeting recording");

    let meeting = state.db.load_meeting(&meeting_id)?;
    let active_profile = current_active_profile(&state)?;
    if active_profile != meeting.transcription_profile {
        return Err(format!(
            "Active transcription profile '{}' / '{}' does not match this meeting's profile '{}' / '{}'",
            active_profile.engine_label,
            active_profile.model_label,
            meeting.transcription_profile.engine_label,
            meeting.transcription_profile.model_label,
        ));
    }

    let accumulator = MeetingAccumulator {
        id: meeting.id,
        title: meeting.title,
        existing_segments: meeting.segments,
        new_segments: Vec::new(),
        recording_sessions: meeting.recording_sessions,
        session_started_at: chrono::Utc::now(),
        transcription_profile: meeting.transcription_profile,
        summary: meeting.summary,
        summary_is_stale: meeting.summary_is_stale,
        summary_model: meeting.summary_model,
        summary_generated_at: meeting.summary_generated_at,
    };

    start_meeting_session(&state, accumulator, channel)?;

    info!(meeting_id = %meeting_id, "Meeting recording resumed");
    Ok(())
}

/// Stop meeting recording and save transcript.
#[tauri::command]
#[specta::specta]
pub fn stop_meeting_recording(state: State<'_, AppState>) -> Result<String, String> {
    let is_recording = *state.is_recording.acquire()?;
    if !is_recording {
        return Err("Not recording".into());
    }
    if *state.recording_mode.acquire()? != RecordingMode::Meeting {
        return Err("Meeting recording is not active".into());
    }

    stop_pipeline(&state)?;

    let mut acc_guard = state.meeting_accumulator.acquire()?;
    let meeting = acc_guard.take().ok_or("No meeting accumulator")?;
    let transcript = build_meeting_transcript(meeting, chrono::Utc::now());
    let id = transcript.id.clone();

    state.db.save_meeting(&transcript)?;
    info!(
        id = %id,
        duration = transcript.duration_seconds,
        sessions = transcript.recording_sessions.len(),
        "Meeting recording saved"
    );

    Ok(id)
}

/// Copy text to clipboard and simulate Cmd+V paste
#[tauri::command]
#[specta::specta]
pub fn paste_text(text: String, delay_ms: u64) -> Result<(), String> {
    crate::clipboard::copy_and_paste(&text, delay_ms)
}

/// Save f32 PCM samples (24kHz mono) to a WAV file
fn save_wav(samples: &[f32]) -> Result<PathBuf, String> {
    use crate::constants::SAMPLE_RATE;

    let recordings_dir = crate::constants::app_data_dir().join("recordings");

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

#[cfg(test)]
mod tests {
    use super::build_meeting_transcript;
    use crate::engine::{TranscriptionSegment, default_transcription_profile};
    use crate::state::MeetingAccumulator;
    use crate::transcript::MeetingRecordingSession;
    use chrono::{Duration, Utc};

    #[test]
    fn build_meeting_transcript_appends_resumed_session_and_marks_summary_stale() {
        let first_start = Utc::now();
        let first_end = first_start + Duration::seconds(30);
        let second_start = first_end + Duration::minutes(5);
        let second_end = second_start + Duration::seconds(45);

        let transcript = build_meeting_transcript(
            MeetingAccumulator {
                id: "meeting-1".to_string(),
                title: "Roadmap".to_string(),
                existing_segments: vec![TranscriptionSegment {
                    text: "Existing".to_string(),
                    start_time: 0.0,
                    end_time: 1.0,
                    is_final: true,
                    language: None,
                    confidence: None,
                }],
                new_segments: vec![TranscriptionSegment {
                    text: "Appended".to_string(),
                    start_time: 1.0,
                    end_time: 2.0,
                    is_final: true,
                    language: None,
                    confidence: None,
                }],
                recording_sessions: vec![MeetingRecordingSession::completed(
                    "session-1".to_string(),
                    first_start,
                    first_end,
                    0,
                    1,
                )],
                session_started_at: second_start,
                transcription_profile: default_transcription_profile(),
                summary: Some("Old summary".to_string()),
                summary_is_stale: false,
                summary_model: Some("qwen".to_string()),
                summary_generated_at: Some(first_end),
            },
            second_end,
        );

        assert_eq!(transcript.id, "meeting-1");
        assert_eq!(transcript.recording_sessions.len(), 2);
        assert_eq!(transcript.recording_sessions[1].start_segment_index, 1);
        assert_eq!(transcript.recording_sessions[1].end_segment_index, 2);
        assert_eq!(transcript.segments.len(), 2);
        assert!(transcript.summary_is_stale);
        assert_eq!(transcript.started_at, first_start);
        assert_eq!(transcript.ended_at, Some(second_end));
        assert_eq!(transcript.duration_seconds, 75.0);
    }

    #[test]
    fn build_meeting_transcript_preserves_fresh_summary_when_no_new_segments_arrive() {
        let started_at = Utc::now();
        let ended_at = started_at + Duration::seconds(10);

        let transcript = build_meeting_transcript(
            MeetingAccumulator {
                id: "meeting-2".to_string(),
                title: "Silent Resume".to_string(),
                existing_segments: Vec::new(),
                new_segments: Vec::new(),
                recording_sessions: Vec::new(),
                session_started_at: started_at,
                transcription_profile: default_transcription_profile(),
                summary: Some("Still current".to_string()),
                summary_is_stale: false,
                summary_model: Some("qwen".to_string()),
                summary_generated_at: Some(started_at),
            },
            ended_at,
        );

        assert_eq!(transcript.recording_sessions.len(), 1);
        assert_eq!(transcript.duration_seconds, 10.0);
        assert!(!transcript.summary_is_stale);
    }
}
