use std::sync::Arc;
use std::time::Duration;

use tauri::State;
use tauri::ipc::Channel;
use tracing::info;
use uuid::Uuid;

use crate::constants::STOP_REPLY_TIMEOUT_SECS;
use crate::lock_ext::MutexExt;
use crate::pipeline::{SegmentCallback, SessionConfig};
use crate::state::{AppState, AudioCommand, MeetingAccumulator};
use crate::state_machine::StateAction;
use crate::transcript::{MeetingRecordingSession, MeetingTranscript};

fn next_audio_session_id(state: &AppState) -> Result<u64, String> {
    let mut guard = state.next_audio_session_id.acquire()?;
    *guard += 1;
    Ok(*guard)
}

fn current_active_profile(state: &AppState) -> Result<crate::engine::TranscriptionProfile, String> {
    state
        .current_machine_state()?
        .active_profile()
        .cloned()
        .ok_or_else(|| "No active transcription profile".to_string())
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

    if let Err(error) = start_pipeline(state, on_segment, PipelineMode::Meeting) {
        let mut acc = state.meeting_accumulator.acquire()?;
        *acc = None;
        return Err(error);
    }

    Ok(())
}

/// Also used by the engine actor to salvage a meeting when its recording
/// session aborts mid-way.
pub(crate) fn build_meeting_transcript(
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
        edited_transcript: None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PipelineMode {
    Dictation,
    Meeting,
}

/// Shared logic for starting a recording session.
/// Validates preconditions, starts the actor session (which drains stale
/// audio, resets the engine, and builds filter chains), then begins audio capture.
fn start_pipeline(
    state: &AppState,
    on_segment: SegmentCallback,
    mode: PipelineMode,
) -> Result<u64, String> {
    let machine = state.current_machine_state()?;
    if !machine.is_model_ready() {
        return Err("Model not loaded".into());
    }
    if machine.active_profile().is_none() {
        return Err("No active transcription profile".into());
    }
    if machine.is_recording() {
        return Err("Already recording".into());
    }

    let session_id = next_audio_session_id(state)?;

    // Snapshot settings and dictionary; the actor builds filter chains on its
    // own thread to keep ONNX/Metal work off the command thread.
    let settings = crate::settings::AppSettings::load(&state.db)?;
    let config = SessionConfig {
        pipeline_config: settings.pipeline_config(),
        dictionary_entries: state.db.list_dictionary_entries()?,
    };

    // Meetings also capture system audio (the other participants) when the
    // setting is on and the OS supports Core Audio taps.
    let capture_system_audio = mode == PipelineMode::Meeting
        && settings.capture_system_audio
        && crate::platform::system_audio_capture_supported();

    // The actor replies once the engine is reset and ready for audio.
    let info = state
        .engine_actor
        .start_session(session_id, config, on_segment)?;

    state
        .audio_cmd_sender
        .send(AudioCommand::Start {
            session_id,
            target_sample_rate: info.audio.sample_rate_hz,
            mic_gain: info.mic_gain,
            capture_system_audio,
        })
        .map_err(|e| format!("Audio start: {e}"))?;

    Ok(session_id)
}

/// Shared logic for stopping a recording session.
/// Stops audio capture (which emits an EndOfStream marker once its stream is
/// dropped and the resampler flushed), then asks the actor to stop — the actor
/// finishes when that marker arrives, so the drain is event-ordered, not timed.
fn stop_pipeline(state: &AppState) -> Result<(), String> {
    // Audio thread drops the cpal stream, flushes the resampler tail, and
    // sends EndOfStream as the final message of this session.
    state
        .audio_cmd_sender
        .send(AudioCommand::Stop)
        .map_err(|e| format!("Audio stop: {e}"))?;

    // The actor drains everything up to EndOfStream and flushes the engine.
    // The timeout is last-resort safety; callers complete state transitions
    // even when this errors.
    let summary = state
        .engine_actor
        .stop_session(Duration::from_secs(STOP_REPLY_TIMEOUT_SECS))?;
    if crate::debug::transcription_debug_enabled() {
        tracing::debug!(
            frames = summary.frames_processed,
            skipped = summary.skipped_chunks,
            "Session stopped"
        );
    }

    Ok(())
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

    let session_id = start_pipeline(&state, on_segment, PipelineMode::Dictation)?;

    // Transition state machine
    state.apply_transition(StateAction::StartDictation { session_id })?;

    info!("Streaming transcription active");
    Ok(())
}

/// Stop streaming transcription.
#[tauri::command]
#[specta::specta]
pub fn stop_transcription(state: State<'_, AppState>) -> Result<(), String> {
    if !state.current_machine_state()?.is_recording() {
        return Err("Not recording".into());
    }

    // Transition to Stopping
    state.apply_transition(StateAction::StopRecording)?;

    // Pipeline stop can fail (e.g. drain timeout) but we MUST complete the
    // state transition — otherwise the machine stays stuck in Stopping.
    if let Err(e) = stop_pipeline(&state) {
        tracing::warn!("Pipeline stop failed: {e}");
    }

    // Transition to Ready even if pipeline stop failed
    state.apply_transition(StateAction::StopComplete)?;

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

    let meeting_id = Uuid::new_v4().to_string();

    let accumulator = MeetingAccumulator {
        id: meeting_id.clone(),
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

    // Get session_id from the pipeline (it was allocated inside start_pipeline)
    let session_id = *state.next_audio_session_id.acquire()?;
    state.apply_transition(StateAction::StartMeeting {
        session_id,
        meeting_id,
    })?;

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
        id: meeting.id.clone(),
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

    let session_id = *state.next_audio_session_id.acquire()?;
    state.apply_transition(StateAction::StartMeeting {
        session_id,
        meeting_id: meeting_id.clone(),
    })?;

    info!(meeting_id = %meeting_id, "Meeting recording resumed");
    Ok(())
}

/// Stop meeting recording and save transcript.
#[tauri::command]
#[specta::specta]
pub fn stop_meeting_recording(state: State<'_, AppState>) -> Result<String, String> {
    let machine = state.current_machine_state()?;
    if !machine.is_recording() {
        return Err("Not recording".into());
    }
    if !matches!(machine, crate::state_machine::AppStateMachine::RecordingMeeting { .. }) {
        return Err("Meeting recording is not active".into());
    }

    // Transition to Stopping
    state.apply_transition(StateAction::StopRecording)?;

    // stop_pipeline can fail (e.g. drain timeout) but we MUST complete the
    // state transition regardless — otherwise the machine stays stuck in
    // Stopping and the user can never recover without restarting.
    let pipeline_err = stop_pipeline(&state).err();

    // Transition to Ready even if pipeline stop failed
    state.apply_transition(StateAction::StopComplete)?;

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

    if let Some(err) = pipeline_err {
        tracing::warn!("Pipeline stop failed (meeting saved anyway): {err}");
    }

    Ok(id)
}

/// Copy text to clipboard and simulate Cmd+V paste
#[tauri::command]
#[specta::specta]
pub fn paste_text(text: String, delay_ms: u64) -> Result<(), String> {
    crate::clipboard::copy_and_paste(&text, delay_ms)
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
