use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use crossbeam_channel::Sender;
use tauri::ipc::Channel;
use tauri::{Manager, State};
use tauri_specta::Event;
use tracing::{info, warn};
use uuid::Uuid;

use crate::app_events::MeetingFinalized;
use crate::constants::STOP_REPLY_TIMEOUT_SECS;
use crate::db::Database;
use crate::engine::TranscriptionSegment;
use crate::lock_ext::MutexExt;
use crate::pipeline::{EngineActorHandle, SegmentCallback, SessionConfig};
use crate::state::{AppState, AudioCommand, MeetingAccumulator};
use crate::state_machine::StateAction;
use crate::transcript::{MeetingRecordingSession, MeetingTranscript};

/// Flush accumulated meeting segments to the DB once this many have piled up
/// since the last flush. Segments are word-level, so this is a few seconds of
/// speech — the upper bound on what a crash can lose.
const MEETING_FLUSH_THRESHOLD: usize = 16;

/// Build a header-only transcript (no segments) for `upsert_meeting_header`.
/// `started_at` follows the first recording session on resume, else this
/// session's start — matching `build_meeting_transcript`.
fn meeting_header(acc: &MeetingAccumulator) -> MeetingTranscript {
    let started_at = acc
        .recording_sessions
        .first()
        .map(|s| s.started_at)
        .unwrap_or(acc.session_started_at);
    MeetingTranscript {
        id: acc.id.clone(),
        title: acc.title.clone(),
        started_at,
        ended_at: None,
        duration_seconds: 0.0,
        transcription_profile: acc.transcription_profile.clone(),
        recording_sessions: acc.recording_sessions.clone(),
        segments: Vec::new(),
        summary: acc.summary.clone(),
        summary_is_stale: acc.summary_is_stale,
        summary_model: acc.summary_model.clone(),
        summary_generated_at: acc.summary_generated_at,
        edited_transcript: None,
        notes: acc.notes.clone(),
    }
}

/// The per-segment callback for meetings: stream to the frontend, accumulate in
/// memory, and periodically flush a batch to the DB for crash durability. Runs
/// on the engine-actor thread (not the realtime audio callback), so the batched
/// DB write is acceptable. The accumulator lock is dropped before the write.
fn build_meeting_on_segment(
    channel: Channel<TranscriptionSegment>,
    accumulator: Arc<Mutex<Option<MeetingAccumulator>>>,
    db: Arc<Database>,
) -> SegmentCallback {
    Box::new(move |seg| {
        let _ = channel.send(seg.clone());

        let batch = {
            let Ok(mut guard) = accumulator.lock() else {
                return;
            };
            let Some(meeting) = guard.as_mut() else {
                return;
            };
            meeting.new_segments.push(seg);
            let unpersisted = meeting.new_segments.len() - meeting.persisted_new_count;
            if unpersisted < MEETING_FLUSH_THRESHOLD {
                None
            } else {
                let start = (meeting.existing_segments.len() + meeting.persisted_new_count) as i64;
                let slice = meeting.new_segments[meeting.persisted_new_count..].to_vec();
                meeting.persisted_new_count = meeting.new_segments.len();
                Some((meeting.id.clone(), slice, start))
            }
        };

        if let Some((id, segments, start)) = batch
            && let Err(e) = db.append_segments(&id, &segments, start)
        {
            warn!("Incremental meeting segment flush failed: {e}");
        }
    })
}

/// Set up and launch a meeting recording (new or resumed). Persists the header
/// up front, stores the accumulator, then starts the engine session off-thread.
async fn launch_meeting(
    state: &AppState,
    accumulator: MeetingAccumulator,
    channel: Channel<TranscriptionSegment>,
) -> Result<u64, String> {
    let session_id = next_audio_session_id(state)?;

    // Persist the header before any segments so a crash leaves a recoverable
    // row (ended_at IS NULL) and segment FK targets exist.
    state.db.upsert_meeting_header(&meeting_header(&accumulator))?;

    {
        let mut acc = state.meeting_accumulator.acquire()?;
        *acc = Some(accumulator);
    }

    let on_segment = build_meeting_on_segment(
        channel,
        Arc::clone(&state.meeting_accumulator),
        Arc::clone(&state.db),
    );

    let actor = Arc::clone(&state.engine_actor);
    let audio = state.audio_cmd_sender.clone();
    let db = Arc::clone(&state.db);
    let acc = Arc::clone(&state.meeting_accumulator);
    let res = tauri::async_runtime::spawn_blocking(move || {
        start_pipeline_blocking(&actor, &audio, &db, session_id, PipelineMode::Meeting, on_segment)
    })
    .await
    .map_err(|e| format!("Join start task: {e}"))?;

    if let Err(error) = res {
        if let Ok(mut acc) = acc.lock() {
            *acc = None;
        }
        return Err(error);
    }

    Ok(session_id)
}

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
        notes: meeting.notes,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PipelineMode {
    Dictation,
    Meeting,
}

/// Blocking core of starting a recording session, run via `spawn_blocking` so
/// the long crossbeam reply wait (engine reset + filter-chain build) never
/// blocks the Tauri command thread / window event loop. Preconditions and
/// session-id allocation are done on the command thread before this runs.
fn start_pipeline_blocking(
    engine_actor: &EngineActorHandle,
    audio_cmd_sender: &Sender<AudioCommand>,
    db: &Database,
    session_id: u64,
    mode: PipelineMode,
    on_segment: SegmentCallback,
) -> Result<(), String> {
    // Snapshot settings and dictionary; the actor builds filter chains on its
    // own thread to keep ONNX/Metal work off the command thread.
    let settings = crate::settings::AppSettings::load(db)?;
    let config = SessionConfig {
        pipeline_config: settings.pipeline_config(),
        dictionary_entries: db.list_dictionary_entries()?,
    };

    // Meetings also capture system audio (the other participants) when the
    // setting is on and the OS supports Core Audio taps.
    let capture_system_audio = mode == PipelineMode::Meeting
        && settings.capture_system_audio
        && crate::platform::system_audio_capture_supported();

    // The actor replies once the engine is reset and ready for audio.
    let info = engine_actor.start_session(session_id, config, on_segment)?;

    audio_cmd_sender
        .send(AudioCommand::Start {
            session_id,
            target_sample_rate: info.audio.sample_rate_hz,
            mic_gain: info.mic_gain,
            capture_system_audio,
        })
        .map_err(|e| format!("Audio start: {e}"))?;

    Ok(())
}

/// Blocking core of stopping a recording session, run via `spawn_blocking`.
/// Stops audio capture (which emits an EndOfStream marker once its stream is
/// dropped and the resampler flushed), then asks the actor to stop — the actor
/// finishes when that marker arrives, so the drain is event-ordered, not timed.
fn stop_pipeline_blocking(
    engine_actor: &EngineActorHandle,
    audio_cmd_sender: &Sender<AudioCommand>,
) -> Result<(), String> {
    // Audio thread drops the cpal stream, flushes the resampler tail, and
    // sends EndOfStream as the final message of this session.
    audio_cmd_sender
        .send(AudioCommand::Stop)
        .map_err(|e| format!("Audio stop: {e}"))?;

    // The actor drains everything up to EndOfStream and flushes the engine.
    // The timeout is last-resort safety; callers complete state transitions
    // even when this errors.
    let summary = engine_actor.stop_session(Duration::from_secs(STOP_REPLY_TIMEOUT_SECS))?;
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
///
/// `async` + `spawn_blocking`: the engine-reset reply can take 0.5–2s, so the
/// blocking wait runs off-thread and the window never freezes.
#[tauri::command]
#[specta::specta]
pub async fn start_transcription(
    state: State<'_, AppState>,
    channel: Channel<crate::engine::TranscriptionSegment>,
) -> Result<(), String> {
    info!("Starting streaming transcription");

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
    let session_id = next_audio_session_id(&state)?;

    let on_segment: SegmentCallback = Box::new(move |seg| {
        let _ = channel.send(seg);
    });

    let actor = Arc::clone(&state.engine_actor);
    let audio = state.audio_cmd_sender.clone();
    let db = Arc::clone(&state.db);
    tauri::async_runtime::spawn_blocking(move || {
        start_pipeline_blocking(&actor, &audio, &db, session_id, PipelineMode::Dictation, on_segment)
    })
    .await
    .map_err(|e| format!("Join start task: {e}"))??;

    state.apply_transition(StateAction::StartDictation { session_id })?;

    info!("Streaming transcription active");
    Ok(())
}

/// Stop streaming transcription.
///
/// Awaits the drain so the frontend's assembled transcript (used for clipboard
/// and dictation history) is complete before this resolves. The drain runs in
/// `spawn_blocking`, so awaiting it does not freeze the window.
#[tauri::command]
#[specta::specta]
pub async fn stop_transcription(state: State<'_, AppState>) -> Result<(), String> {
    if !state.current_machine_state()?.is_recording() {
        return Err("Not recording".into());
    }

    // Transition to Stopping
    state.apply_transition(StateAction::StopRecording)?;

    let actor = Arc::clone(&state.engine_actor);
    let audio = state.audio_cmd_sender.clone();
    // Pipeline stop can fail (e.g. drain timeout) but we MUST complete the
    // state transition — otherwise the machine stays stuck in Stopping.
    let stop_result = tauri::async_runtime::spawn_blocking(move || {
        stop_pipeline_blocking(&actor, &audio)
    })
    .await
    .map_err(|e| format!("Join stop task: {e}"))?;
    if let Err(e) = stop_result {
        warn!("Pipeline stop failed: {e}");
    }

    // Transition to Ready even if pipeline stop failed
    state.apply_transition(StateAction::StopComplete)?;

    info!("Streaming transcription stopped");
    Ok(())
}

/// Start meeting recording with live transcription.
#[tauri::command]
#[specta::specta]
pub async fn start_meeting_recording(
    state: State<'_, AppState>,
    title: String,
    channel: Channel<crate::engine::TranscriptionSegment>,
) -> Result<(), String> {
    info!(title = %title, "Starting meeting recording");

    let machine = state.current_machine_state()?;
    if !machine.is_model_ready() {
        return Err("Model not loaded".into());
    }
    if machine.is_recording() {
        return Err("Already recording".into());
    }

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
        notes: None,
        persisted_new_count: 0,
    };

    let session_id = launch_meeting(&state, accumulator, channel).await?;

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
pub async fn resume_meeting_recording(
    state: State<'_, AppState>,
    meeting_id: String,
    channel: Channel<crate::engine::TranscriptionSegment>,
) -> Result<(), String> {
    info!(meeting_id = %meeting_id, "Resuming meeting recording");

    let machine = state.current_machine_state()?;
    if !machine.is_model_ready() {
        return Err("Model not loaded".into());
    }
    if machine.is_recording() {
        return Err("Already recording".into());
    }

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
        notes: meeting.notes,
        persisted_new_count: 0,
    };

    let session_id = launch_meeting(&state, accumulator, channel).await?;

    state.apply_transition(StateAction::StartMeeting {
        session_id,
        meeting_id: meeting_id.clone(),
    })?;

    info!(meeting_id = %meeting_id, "Meeting recording resumed");
    Ok(())
}

/// Stop meeting recording and save transcript.
///
/// Decoupled stop: transitions to Stopping, returns the (already-known) meeting
/// id immediately, and drains + saves in the background. Segments were persisted
/// incrementally during the meeting, so the detail view can render right away
/// and reconcile when the `MeetingFinalized` event fires.
#[tauri::command]
#[specta::specta]
pub async fn stop_meeting_recording(state: State<'_, AppState>) -> Result<String, String> {
    let machine = state.current_machine_state()?;
    if !machine.is_recording() {
        return Err("Not recording".into());
    }
    if !matches!(machine, crate::state_machine::AppStateMachine::RecordingMeeting { .. }) {
        return Err("Meeting recording is not active".into());
    }

    // Peek the id (without taking the accumulator) so we can return it now.
    let meeting_id = state
        .meeting_accumulator
        .acquire()?
        .as_ref()
        .map(|m| m.id.clone())
        .ok_or("No meeting accumulator")?;

    // Fetch the handle BEFORE transitioning: the background task needs it to
    // complete the stop, so if it's somehow missing we must fail before leaving
    // the machine stuck in Stopping.
    let app = state.app_handle()?;

    // Transition to Stopping immediately so the UI can show "Finalizing…".
    state.apply_transition(StateAction::StopRecording)?;

    // Finish off-thread: drain the engine, save the authoritative transcript,
    // then complete the transition and notify the frontend.
    let id_for_task = meeting_id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();

        let pipeline_err = stop_pipeline_blocking(&state.engine_actor, &state.audio_cmd_sender).err();

        // Authoritative full save from the in-memory accumulator (overwrites the
        // incrementally-persisted rows with the complete, finalized transcript).
        if let Ok(mut guard) = state.meeting_accumulator.lock()
            && let Some(meeting) = guard.take()
        {
            let transcript = build_meeting_transcript(meeting, chrono::Utc::now());
            if let Err(e) = state.db.save_meeting(&transcript) {
                tracing::error!(id = %transcript.id, "Failed to save meeting: {e}");
            } else {
                info!(
                    id = %transcript.id,
                    duration = transcript.duration_seconds,
                    sessions = transcript.recording_sessions.len(),
                    "Meeting recording saved"
                );
            }
        }

        // Always complete the transition, even if drain/save failed, so the
        // machine never gets stuck in Stopping.
        if let Err(e) = state.apply_transition(StateAction::StopComplete) {
            warn!("Failed to complete stop transition: {e}");
        }
        if let Some(err) = pipeline_err {
            warn!("Pipeline stop failed (meeting saved anyway): {err}");
        }

        let _ = MeetingFinalized { id: id_for_task }.emit(&app);
    });

    Ok(meeting_id)
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
                notes: None,
                persisted_new_count: 0,
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
                notes: None,
                persisted_new_count: 0,
            },
            ended_at,
        );

        assert_eq!(transcript.recording_sessions.len(), 1);
        assert_eq!(transcript.duration_seconds, 10.0);
        assert!(!transcript.summary_is_stale);
    }
}
