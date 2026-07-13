use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use crossbeam_channel::Sender;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use tauri_specta::Event;
use tracing::{info, warn};
use uuid::Uuid;

use crate::app_events::{MeetingFinalized, SystemWokeUp};
use crate::constants::STOP_REPLY_TIMEOUT_SECS;
use crate::db::Database;
use crate::engine::TranscriptionSegment;
use crate::lock_ext::MutexExt;
use crate::pipeline::{EngineActorHandle, SegmentCallback, SessionConfig};
use crate::settings::AppSettings;
use crate::state::{AppState, AudioCommand, MeetingAccumulator};
use crate::state_machine::{AppStateMachine, StateAction};
use crate::transcript::{MeetingCalendarContext, MeetingTranscript};

/// Flush accumulated meeting segments to the DB once this many have piled up
/// since the last flush. Segments are word-level, so this is a few seconds of
/// speech — the upper bound on what a crash can lose.
const MEETING_FLUSH_THRESHOLD: usize = 16;

/// Build a header-only transcript (no segments) for `upsert_meeting_header`.
/// `started_at` follows the first recording session on resume, else this
/// session's start — matching `MeetingAccumulator::into_transcript`.
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
        structured_summary: acc.structured_summary.clone(),
        edited_transcript: None,
        notes: acc.notes.clone(),
        calendar_event_id: acc.calendar_event_id.clone(),
        participants: acc.participants.clone(),
        // Header-only write; `speakers` is recomputed by `load_meeting`.
        speakers: Vec::new(),
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
                Some((
                    meeting.id.clone(),
                    slice,
                    start,
                    meeting.persisted_new_count,
                ))
            }
        };

        if let Some((id, segments, start, advanced_to)) = batch
            && let Err(e) = db.append_segments(&id, &segments, start)
        {
            warn!("Incremental meeting segment flush failed: {e}");
            // Roll the counter back so this batch is retried on the next flush
            // instead of being permanently skipped, which would otherwise widen
            // the crash-loss window beyond one batch. This callback runs only on
            // the single engine-actor thread, so nothing else advances
            // persisted_new_count between the write above and this re-lock; the
            // equality check below only guards against the accumulator having
            // been taken (and possibly replaced) by a concurrent stop.
            if let Ok(mut guard) = accumulator.lock()
                && let Some(meeting) = guard.as_mut()
                && meeting.persisted_new_count == advanced_to
            {
                meeting.persisted_new_count -= segments.len();
            }
        }
    })
}

/// Set up and launch a meeting recording (new or resumed). Persists the header
/// up front, stores the accumulator, then starts the engine session off-thread.
async fn launch_meeting(
    state: &AppState,
    accumulator: MeetingAccumulator,
    event_description: Option<String>,
    channel: Channel<TranscriptionSegment>,
) -> Result<u64, String> {
    // Any recording starting now (whether the user resumed by hand or
    // started something new) makes a stale sleep-paused bookkeeping entry
    // meaningless — clear it so a later wake never misreports an
    // already-handled meeting as needing a resume prompt.
    let _ = state.take_sleep_paused_meeting();

    let session_id = next_audio_session_id(state)?;

    // Session-scoped transcription hints: participant names plus distinctive
    // jargon from the event title/description. Never persisted.
    let participant_names: Vec<String> = accumulator
        .participants
        .iter()
        .map(|p| p.name.clone())
        .collect();
    let session_terms = crate::filter::session_terms::derive_session_terms(
        &participant_names,
        &[
            accumulator.title.as_str(),
            event_description.as_deref().unwrap_or(""),
        ],
    );

    // This session's position in the meeting's recording_sessions — 0 for a
    // new meeting, len() for a resume — is exactly the on-disk file index a
    // recorder should write to, if recording is on.
    let recording_target = RecordingTarget {
        meeting_id: accumulator.id.clone(),
        session_index: accumulator.recording_sessions.len(),
    };

    // Persist the header before any segments so a crash leaves a recoverable
    // row (ended_at IS NULL) and segment FK targets exist.
    state
        .db
        .upsert_meeting_header(&meeting_header(&accumulator))?;

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
        start_pipeline_blocking(
            &actor,
            &audio,
            &db,
            session_id,
            PipelineMode::Meeting,
            session_terms,
            Some(recording_target),
            on_segment,
        )
    })
    .await
    .map_err(|e| format!("Join start task: {e}"))?;

    if let Err(error) = res {
        if let Ok(mut acc) = acc.lock() {
            *acc = None;
        }
        // The header row upserted above is now orphaned (ended_at IS NULL) with
        // no recording in progress: recovery is safe to run here immediately,
        // rather than waiting for the next app restart, because clearing the
        // accumulator above guarantees no meeting is mid-recording. This either
        // deletes an empty new-meeting shell or finalizes a resumed meeting from
        // its already-persisted segments.
        if let Err(e) = state.db.recover_unfinished_meetings() {
            warn!("Meeting recovery after failed start failed: {e}");
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum PipelineMode {
    Dictation,
    Meeting,
}

/// Identifies where a meeting recording session's audio file belongs, if
/// the retention setting turns out to be on. Resolved before the session
/// starts (`launch_meeting` knows the meeting id and the session's position
/// in `recording_sessions`); whether to actually record is decided inside
/// `start_pipeline_blocking` once settings are loaded.
struct RecordingTarget {
    meeting_id: String,
    session_index: usize,
}

/// Blocking core of starting a recording session, run via `spawn_blocking` so
/// the long crossbeam reply wait (engine reset + filter-chain build) never
/// blocks the Tauri command thread / window event loop. Preconditions and
/// session-id allocation are done on the command thread before this runs.
#[allow(clippy::too_many_arguments)]
fn start_pipeline_blocking(
    engine_actor: &EngineActorHandle,
    audio_cmd_sender: &Sender<AudioCommand>,
    db: &Database,
    session_id: u64,
    mode: PipelineMode,
    session_terms: Vec<String>,
    recording_target: Option<RecordingTarget>,
    on_segment: SegmentCallback,
) -> Result<(), String> {
    // Snapshot settings and dictionary; the actor builds filter chains on its
    // own thread to keep ONNX/Metal work off the command thread.
    let settings = crate::settings::AppSettings::load(db)?;

    // Meetings also capture system audio (the other participants) when the
    // setting is on and the OS supports Core Audio taps.
    let capture_system_audio = mode == PipelineMode::Meeting
        && settings.capture_system_audio
        && crate::platform::system_audio_capture_supported();
    // Speaker labelling (Me/Them) needs a distinct system-audio leg AND an
    // engine that can transcribe two batched lanes. Only Kyutai can today; other
    // engines record meetings as a single mixed stream with no labels. This must
    // match the engine's own capability so capture and the actor agree on
    // whether to split the audio.
    let diarize =
        capture_system_audio && settings.transcription_engine_id == crate::engine::KYUTAI_ENGINE_ID;

    // Auto-stop detection only applies to meetings: dictation sessions are
    // short and user-driven, so "meeting is over" doesn't apply. This is a
    // session-start snapshot; a setting changed mid-meeting only takes effect
    // on the next meeting.
    let idle_config = (mode == PipelineMode::Meeting && settings.meeting_autostop_enabled).then(
        || crate::pipeline::MeetingIdleConfig {
            silence_threshold: Some(Duration::from_secs(u64::from(
                settings.meeting_autostop_minutes * 60,
            ))),
            max_duration: Some(Duration::from_secs(u64::from(
                settings.meeting_max_duration_minutes * 60,
            ))),
        },
    );

    let config = SessionConfig {
        pipeline_config: settings.pipeline_config(),
        dictionary_entries: db.list_dictionary_entries()?,
        session_terms,
        diarize,
        idle_config,
    };

    // The actor replies once the engine is reset and ready for audio.
    let info = engine_actor.start_session(session_id, config, on_segment)?;

    // Recording is opt-in and meeting-only; resolve the actual path only
    // once the retention setting is known (a `RecordingTarget` just means
    // "this session's audio would live here if recording is on").
    let record_path = if mode == PipelineMode::Meeting
        && settings.meeting_audio_retention != crate::settings::MeetingAudioRetention::Off
    {
        recording_target
            .map(|target| crate::audio::recorder::session_path(&target.meeting_id, target.session_index))
    } else {
        None
    };

    audio_cmd_sender
        .send(AudioCommand::Start {
            session_id,
            target_sample_rate: info.audio.sample_rate_hz,
            mic_gain: info.mic_gain,
            capture_system_audio,
            diarize,
            record_path,
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

/// Running accumulation for the pill's live-text preview: the cumulative
/// final-segment text (mirrors the frontend's own transcript join logic) and
/// when it was last emitted, for throttling.
#[derive(Default)]
struct DictationLiveTextState {
    accumulated: String,
    last_emit: Option<std::time::Instant>,
}

/// The per-segment callback for dictation: forward every segment to the main
/// window's channel as before, and additionally accumulate final segments'
/// text to emit a throttled `DictationLiveText` sample to the (separate)
/// pill webview. Meetings do not go through this path — their own
/// `build_meeting_on_segment` never touches live text.
fn build_dictation_on_segment(
    app: AppHandle,
    channel: Channel<crate::engine::TranscriptionSegment>,
) -> SegmentCallback {
    let live_text = Mutex::new(DictationLiveTextState::default());
    Box::new(move |seg| {
        let is_final = seg.is_final;
        let text = seg.text.clone();
        let _ = channel.send(seg);

        if !is_final {
            return;
        }
        let Ok(mut state) = live_text.lock() else {
            return;
        };
        if !state.accumulated.is_empty() && !state.accumulated.ends_with(' ') && !text.starts_with(' ') {
            state.accumulated.push(' ');
        }
        state.accumulated.push_str(&text);

        let now = std::time::Instant::now();
        if crate::pill::should_emit_live_text(state.last_emit, now, crate::pill::LIVE_TEXT_MIN_INTERVAL) {
            state.last_emit = Some(now);
            let tail = crate::pill::live_text_tail(&state.accumulated, crate::pill::LIVE_TEXT_MAX_CHARS);
            drop(state);
            let _ = crate::app_events::DictationLiveText { text: tail }.emit(&app);
        }
    })
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

    let on_segment: SegmentCallback = match state.app_handle() {
        Ok(app) => build_dictation_on_segment(app, channel),
        // No AppHandle (should not happen once the app is running): keep
        // forwarding segments to the main window even without live text.
        Err(_) => Box::new(move |seg| {
            let _ = channel.send(seg);
        }),
    };

    let actor = Arc::clone(&state.engine_actor);
    let audio = state.audio_cmd_sender.clone();
    let db = Arc::clone(&state.db);
    tauri::async_runtime::spawn_blocking(move || {
        start_pipeline_blocking(
            &actor,
            &audio,
            &db,
            session_id,
            PipelineMode::Dictation,
            Vec::new(),
            None,
            on_segment,
        )
    })
    .await
    .map_err(|e| format!("Join start task: {e}"))??;

    state.apply_transition(StateAction::StartDictation { session_id })?;

    if let Ok(settings) = AppSettings::load(&state.db) {
        crate::audio::feedback::play_dictation_feedback(
            &settings,
            crate::audio::feedback::DictationFeedbackKind::Start,
        );
    }

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
    let was_dictation = matches!(
        state.current_machine_state()?,
        AppStateMachine::RecordingDictation { .. }
    );
    if !state.current_machine_state()?.is_recording() {
        return Err("Not recording".into());
    }

    // Transition to Stopping
    state.apply_transition(StateAction::StopRecording)?;

    let actor = Arc::clone(&state.engine_actor);
    let audio = state.audio_cmd_sender.clone();
    // Pipeline stop can fail (e.g. drain timeout) but we MUST complete the
    // state transition — otherwise the machine stays stuck in Stopping.
    let stop_result =
        tauri::async_runtime::spawn_blocking(move || stop_pipeline_blocking(&actor, &audio))
            .await
            .map_err(|e| format!("Join stop task: {e}"))?;
    if let Err(e) = stop_result {
        warn!("Pipeline stop failed: {e}");
    }

    // Transition to Ready even if pipeline stop failed
    state.apply_transition(StateAction::StopComplete)?;

    // Clear the pill's live-text preview now that the session is over — the
    // dictation on_segment closure (and its accumulated text) is dropped
    // with the session, so nothing else will do this.
    if was_dictation
        && let Ok(app) = state.app_handle()
    {
        let _ = crate::app_events::DictationLiveText { text: String::new() }.emit(&app);
    }

    if was_dictation
        && let Ok(settings) = AppSettings::load(&state.db)
    {
        crate::audio::feedback::play_dictation_feedback(
            &settings,
            crate::audio::feedback::DictationFeedbackKind::Stop,
        );
    }

    info!("Streaming transcription stopped");
    Ok(())
}

/// Start meeting recording with live transcription.
#[tauri::command]
#[specta::specta]
pub async fn start_meeting_recording(
    state: State<'_, AppState>,
    title: String,
    calendar: Option<MeetingCalendarContext>,
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
    // The machine only flips to recording_meeting AFTER the (possibly slow)
    // session launch, so the is_recording() check above races a second start.
    // The accumulator is set at the very start of launch_meeting, so its
    // presence is the reliable "a meeting is already starting" signal — without
    // this, a concurrent start's failure path orphans the live accumulator.
    if state.meeting_accumulator.acquire()?.is_some() {
        return Err("A meeting is already starting or recording".into());
    }

    let meeting_id = Uuid::new_v4().to_string();
    let (calendar_event_id, participants, event_description) = match calendar {
        Some(context) => (
            Some(context.event_id),
            context.participants,
            context.description,
        ),
        None => (None, Vec::new(), None),
    };
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
        structured_summary: None,
        notes: None,
        calendar_event_id,
        participants,
        persisted_new_count: 0,
    };

    let session_id = launch_meeting(&state, accumulator, event_description, channel).await?;

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
    // See start_meeting_recording: a live accumulator means a start is already
    // in flight, so reject before launch can orphan it.
    if state.meeting_accumulator.acquire()?.is_some() {
        return Err("A meeting is already starting or recording".into());
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
        structured_summary: meeting.structured_summary,
        notes: meeting.notes,
        calendar_event_id: meeting.calendar_event_id,
        participants: meeting.participants,
        persisted_new_count: 0,
    };

    let session_id = launch_meeting(&state, accumulator, None, channel).await?;

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
    if !matches!(
        machine,
        crate::state_machine::AppStateMachine::RecordingMeeting { .. }
    ) {
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

        // Only the fallible drain+save is guarded: if it panics (e.g. an engine
        // or DB driver bug), the transition and event emit below still must run
        // so the machine never gets stuck in Stopping. Segments already flushed
        // incrementally during the meeting are not lost even if this panics.
        let drain_and_save = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let pipeline_err =
                stop_pipeline_blocking(&state.engine_actor, &state.audio_cmd_sender).err();

            // Authoritative full save from the in-memory accumulator (overwrites the
            // incrementally-persisted rows with the complete, finalized transcript).
            if let Ok(mut guard) = state.meeting_accumulator.lock()
                && let Some(meeting) = guard.take()
            {
                let transcript = meeting.into_transcript(chrono::Utc::now());
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

            pipeline_err
        }));

        let pipeline_err = match drain_and_save {
            Ok(pipeline_err) => pipeline_err,
            Err(_) => {
                tracing::error!("Meeting stop task panicked during drain/save");
                None
            }
        };

        // Always complete the transition, even if drain/save failed or panicked,
        // so the machine never gets stuck in Stopping.
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

/// Insert dictation text into the active app (clipboard paste or simulated typing).
#[tauri::command]
#[specta::specta]
pub fn paste_text(
    text: String,
    delay_ms: u64,
    method: crate::settings::PasteMethod,
) -> Result<(), String> {
    crate::clipboard::paste_text(&text, delay_ms, method)
}

/// Called from the `NSWorkspace` will-sleep observer (installed in `power.rs`
/// during setup) on the main thread. If a meeting recording is active, stop
/// it through the exact same path a user-initiated stop takes — segments are
/// already persisted incrementally, so the background drain+save loses
/// nothing — and remember its id so the frontend can offer to resume after
/// wake. A dictation session is just stopped the normal way too; it's
/// ephemeral, and the existing stop path already saves the partial
/// transcript to history.
///
/// Must not block: the drain runs in a spawned task, not inline, since this
/// fires from the AppKit notification callback on the main thread.
pub fn handle_system_will_sleep(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Ok(machine) = state.current_machine_state() else {
        return;
    };
    if !machine.is_recording() {
        return;
    }

    let meeting_id = match &machine {
        AppStateMachine::RecordingMeeting { meeting_id, .. } => Some(meeting_id.clone()),
        _ => None,
    };
    if let Some(id) = &meeting_id {
        state.set_sleep_paused_meeting(id.clone());
    }

    info!("System will sleep: stopping the active recording session");
    let app = app.clone();
    let is_meeting = meeting_id.is_some();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        // `Stopping` also satisfies `is_recording()`, so a sleep landing
        // right as a user-initiated stop is already in flight harmlessly
        // no-ops here (the command below rejects a non-matching state).
        let result = if is_meeting {
            stop_meeting_recording(state).await.map(|_| ())
        } else {
            stop_transcription(state).await
        };
        if let Err(e) = result {
            warn!("Sleep-triggered stop failed: {e}");
        }
    });
}

/// Called from the `NSWorkspace` did-wake observer on the main thread.
/// Just notifies the frontend — resuming a paused meeting needs a frontend
/// segment channel, so the backend cannot resume on its own.
pub fn handle_system_did_wake(app: &AppHandle) {
    info!("System woke up");
    let _ = SystemWokeUp.emit(app);
}

/// Return and clear the meeting id paused by the system-sleep handler, if
/// any. The frontend calls this on `SystemWokeUp` (and again on webview
/// visibility change, belt and braces) to decide whether to offer/auto-start
/// a resume.
#[tauri::command]
#[specta::specta]
pub fn take_sleep_paused_meeting(state: State<'_, AppState>) -> Option<String> {
    state.take_sleep_paused_meeting()
}

#[cfg(test)]
mod tests {
    use super::{MEETING_FLUSH_THRESHOLD, build_meeting_on_segment};
    use crate::engine::{TranscriptionSegment, default_transcription_profile};
    use crate::state::MeetingAccumulator;
    use crate::test_helpers::fixtures::test_db;
    use chrono::Utc;
    use std::sync::{Arc, Mutex};
    use tauri::ipc::Channel;

    #[test]
    fn build_meeting_on_segment_rolls_back_persisted_count_on_flush_failure() {
        // No upsert_meeting_header call for this id: the meetings row does not
        // exist, so append_segments fails on the segments.meeting_id foreign
        // key, exercising the same failure the fix guards against.
        let (db, _dir) = test_db();
        let db = Arc::new(db);

        let accumulator = Arc::new(Mutex::new(Some(MeetingAccumulator {
            id: "missing-meeting".to_string(),
            title: "Ghost".to_string(),
            existing_segments: Vec::new(),
            new_segments: Vec::new(),
            recording_sessions: Vec::new(),
            session_started_at: Utc::now(),
            transcription_profile: default_transcription_profile(),
            summary: None,
            summary_is_stale: false,
            summary_model: None,
            summary_generated_at: None,
            structured_summary: None,
            notes: None,
            calendar_event_id: None,
            participants: Vec::new(),
            persisted_new_count: 0,
        })));

        let channel: Channel<TranscriptionSegment> = Channel::new(|_| Ok(()));
        let on_segment = build_meeting_on_segment(channel, Arc::clone(&accumulator), db);

        for i in 0..MEETING_FLUSH_THRESHOLD {
            on_segment(TranscriptionSegment {
                text: format!("segment {i}"),
                start_time: i as f64,
                end_time: i as f64 + 1.0,
                is_final: true,
                language: None,
                confidence: None,
                speaker: None,
            });
        }

        let guard = accumulator.lock().unwrap();
        let meeting = guard.as_ref().unwrap();
        assert_eq!(meeting.new_segments.len(), MEETING_FLUSH_THRESHOLD);
        assert_eq!(
            meeting.persisted_new_count, 0,
            "flush failed (FK violation) so the batch must be retried, not lost"
        );
    }
}
