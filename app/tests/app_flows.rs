//! Smoke-level end-to-end tests for the two critical lifecycles: dictation
//! round-trip and "meeting stop persists a meeting". Drives the real command
//! functions from `souffle_lib::commands` against a plain `Arc<AppState>`,
//! with a `MockEngine` swapped in for the transcription engine (via the
//! actor's injectable `EngineFactory`) and a temp-file SQLite database. No
//! GPU, no real audio hardware.
//!
//! SOU-191 note: before this ticket, `AppState` was managed inside a
//! `tauri::test::MockRuntime` app because commands took `tauri::State`/
//! `AppHandle`, and `stop_meeting_recording`'s background finalize task
//! specifically could not be driven here — building a real, window-backed
//! `AppHandle<Wry>` needs the platform event loop, which conflicts with the
//! `cargo test` harness's threading model. Now that commands take
//! `Arc<AppState>` directly (owned, 'static, no runtime container), that
//! whole workaround is gone: `stop_meeting_recording` runs as the literal,
//! unmodified command below, same as everything else in this suite.

// `other => panic!("expected X, got {other:?}")` is the assertion itself here:
// a new variant makes the test fail loudly with the state it actually saw,
// which is the property the lint protects, reached another way. Enumerating
// nine variants per assertion would bury it.
#![allow(clippy::wildcard_enum_match_arm)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use tempfile::TempDir;

use souffle_lib::audio::{AudioChunk, AudioMessage};
use souffle_lib::commands;
use souffle_lib::constants::MIMI_FRAME_SIZE;
use souffle_lib::db::Database;
use souffle_lib::engine::mock::MockEngine;
use souffle_lib::engine::{
    TranscriptionEngine, TranscriptionSegment, default_transcription_profile,
};
use souffle_lib::pipeline::EngineActorHandle;
use souffle_lib::progress::ProgressChannel;
use souffle_lib::settings::AppSettings;
use souffle_lib::state::{AppState, AudioCommand};
use souffle_lib::state_machine::{AppStateMachine, StateAction};

/// Spawn the engine actor with a `MockEngine` in place of the real
/// transcription backend, mirroring `pipeline::actor::tests::spawn_with_mock`.
fn spawn_mock_actor(mock: MockEngine) -> (EngineActorHandle, Sender<AudioMessage>) {
    let (audio_tx, audio_rx) = crossbeam_channel::unbounded();
    let cell = std::sync::Mutex::new(Some(mock));
    let actor = EngineActorHandle::spawn(
        audio_rx,
        Arc::new(std::sync::atomic::AtomicU64::new(0)),
        Arc::new(std::sync::Mutex::new(None)),
        Box::new(move |_profile| {
            cell.lock()
                .unwrap()
                .take()
                .map(|m| Box::new(m) as Box<dyn TranscriptionEngine>)
                .ok_or_else(|| "mock engine already taken".to_string())
        }),
    )
    .expect("spawn engine actor");
    (actor, audio_tx)
}

/// Drive the app-level state machine through Idle -> Ready, the same
/// sequence `commands::load_model` runs, and load the mock engine so the
/// actor has something to transcribe with.
fn bring_to_ready(state: &AppState, actor: &EngineActorHandle) {
    let profile = default_transcription_profile();
    actor
        .load_model(profile.clone(), PathBuf::from("/tmp"))
        .expect("mock engine load");

    state
        .apply_transition(StateAction::StartDownload {
            profile: profile.clone(),
        })
        .unwrap();
    state
        .apply_transition(StateAction::DownloadComplete)
        .unwrap();
    state
        .apply_transition(StateAction::StartLoad {
            profile: profile.clone(),
        })
        .unwrap();
    state.apply_transition(StateAction::LoadComplete).unwrap();
}

/// Test harness: everything needed to call commands end-to-end plus the
/// pieces (audio channel, actor, db) needed to drive/inspect them directly.
struct Harness {
    state: Arc<AppState>,
    db: Arc<Database>,
    /// Kept alive via `state.engine_actor` (same `Arc`); nothing in this
    /// test suite needs to touch it directly anymore now that
    /// `stop_meeting_recording` runs unmodified (see the module doc).
    audio_msg_tx: Sender<AudioMessage>,
    /// Kept alive so `AppState::audio_cmd_sender.send(...)` never errors with
    /// "all receivers dropped" — nothing needs to read from it since there is
    /// no real audio-capture thread in this test.
    pub audio_cmd_rx: Receiver<AudioCommand>,
    _tmp: TempDir,
}

fn build_harness(mock: MockEngine) -> Harness {
    let tmp = TempDir::new().expect("tempdir");
    let db = Arc::new(Database::open(&tmp.path().join("test.db")).expect("open test db"));

    // Test-friendly settings: no system-audio capture (keeps the session in
    // single-stream mode, avoiding the diarized dual-lane path), no VAD/text
    // filters (so the mock's fixed segment text round-trips unmodified), no
    // auto-stop timers, no feedback sound playback.
    AppSettings {
        capture_system_audio: false,
        vad_enabled: false,
        filler_removal: false,
        stutter_collapse: false,
        dictionary_correction: false,
        meeting_autostop_enabled: false,
        feedback_sounds_enabled: false,
        ..AppSettings::default()
    }
    .save(&db)
    .expect("save test settings");

    let (actor, audio_msg_tx) = spawn_mock_actor(mock);
    let actor = Arc::new(actor);

    let (audio_cmd_tx, audio_cmd_rx) = crossbeam_channel::unbounded::<AudioCommand>();
    let audio_rms = Arc::new(AtomicU32::new(0f32.to_bits()));

    let state = Arc::new(AppState::new(
        audio_cmd_tx,
        Arc::clone(&actor),
        Arc::clone(&db),
        audio_rms,
    ));
    bring_to_ready(&state, &actor);

    Harness {
        state,
        db,
        audio_msg_tx,
        audio_cmd_rx,
        _tmp: tmp,
    }
}

/// A non-silent chunk carrying exactly one engine frame's worth of samples,
/// tagged for the given session (matches `pipeline::actor::tests::audio_chunk`).
fn audio_chunk(session_id: u64) -> AudioMessage {
    AudioMessage::Chunk(AudioChunk {
        queue_permit: None,
        session_id,
        samples: vec![0.1f32; MIMI_FRAME_SIZE],
        captured_at: std::time::Instant::now(),
        speaker: None,
    })
}

/// `stop_meeting_recording` is a decoupled stop: it returns once the state
/// machine reaches `Stopping`, while the drain+save finishes on a background
/// task. Tests that need the stop fully settled (machine back to `Ready`,
/// the meeting row saved) poll for it instead of assuming it finished
/// synchronously.
async fn wait_until_ready(state: &AppState) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if matches!(
            state.current_machine_state().unwrap(),
            AppStateMachine::Ready { .. }
        ) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("machine did not reach Ready within the deadline");
}

/// A channel that appends every message to a shared `Vec`, so tests can
/// assert on what streamed back.
fn collecting_channel() -> (
    Arc<std::sync::Mutex<Vec<TranscriptionSegment>>>,
    ProgressChannel<TranscriptionSegment>,
) {
    let collected: Arc<std::sync::Mutex<Vec<TranscriptionSegment>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let collected_ref = Arc::clone(&collected);
    let channel = ProgressChannel::new(move |segment| {
        collected_ref.lock().unwrap().push(segment);
    });
    (collected, channel)
}

#[tokio::test]
async fn meeting_stop_persists_meeting() {
    let mock = MockEngine::new().with_transcribe_response(
        Ok(vec![TranscriptionSegment {
            text: "hello meeting".to_string(),
            start_time: 0.0,
            end_time: 1.0,
            is_final: true,
            language: Some("en".to_string()),
            confidence: Some(0.9),
            speaker: None,
        }]),
        1,
    );
    let h = build_harness(mock);
    let state = Arc::clone(&h.state);

    let (collected, channel) = collecting_channel();

    commands::start_meeting_recording(Arc::clone(&state), "Weekly Sync".to_string(), None, channel)
        .await
        .expect("start_meeting_recording");

    let (session_id, meeting_id) = match state.current_machine_state().unwrap() {
        AppStateMachine::RecordingMeeting {
            session_id,
            meeting_id,
            ..
        } => (session_id, meeting_id),
        other => panic!("expected RecordingMeeting after start, got {other:?}"),
    };

    // Feed one synthetic audio frame through the actor's audio channel
    // directly — there's no real AudioCapture thread in this test, so this
    // stands in for the microphone.
    h.audio_msg_tx.send(audio_chunk(session_id)).unwrap();
    h.audio_msg_tx
        .send(AudioMessage::EndOfStream { session_id })
        .unwrap();

    let returned_id = commands::stop_meeting_recording(Arc::clone(&state))
        .await
        .expect("stop_meeting_recording");
    assert_eq!(returned_id, meeting_id);

    // Decoupled stop: the background drain+save runs after the command
    // returns.
    wait_until_ready(&state).await;

    // Segments streamed live during the meeting.
    let streamed = collected.lock().unwrap();
    assert!(
        streamed.iter().any(|s| s.text == "hello meeting"),
        "expected a live-streamed 'hello meeting' segment, got: {:?}",
        streamed.iter().map(|s| &s.text).collect::<Vec<_>>()
    );
    drop(streamed);

    // The meeting row exists with ended_at set and the segment persisted —
    // the actual "stop persists a meeting" assertion.
    let meeting =
        h.db.load_meeting(&meeting_id)
            .expect("load persisted meeting");
    assert_eq!(meeting.title, "Weekly Sync");
    assert!(meeting.ended_at.is_some(), "meeting should be finalized");
    assert!(
        meeting.segments.iter().any(|s| s.text == "hello meeting"),
        "expected the transcribed segment to be persisted, got: {:?}",
        meeting.segments.iter().map(|s| &s.text).collect::<Vec<_>>()
    );

    let list = commands::list_meetings(Arc::clone(&state)).expect("list_meetings");
    assert!(
        list.iter().any(|m| m.id == meeting_id),
        "expected the finalized meeting to show up in list_meetings"
    );
}

#[tokio::test]
async fn dictation_round_trip() {
    let mock = MockEngine::new().with_transcribe_response(
        Ok(vec![TranscriptionSegment {
            text: "hello dictation".to_string(),
            start_time: 0.0,
            end_time: 1.0,
            is_final: true,
            language: Some("en".to_string()),
            confidence: Some(0.9),
            speaker: None,
        }]),
        1,
    );
    let h = build_harness(mock);
    let state = Arc::clone(&h.state);

    let (collected, channel) = collecting_channel();

    commands::start_transcription(Arc::clone(&state), channel, false)
        .await
        .expect("start_transcription");

    let session_id = match state.current_machine_state().unwrap() {
        AppStateMachine::RecordingDictation { session_id, .. } => session_id,
        other => panic!("expected RecordingDictation after start, got {other:?}"),
    };

    h.audio_msg_tx.send(audio_chunk(session_id)).unwrap();
    h.audio_msg_tx
        .send(AudioMessage::EndOfStream { session_id })
        .unwrap();

    commands::stop_transcription(Arc::clone(&state))
        .await
        .expect("stop_transcription");

    assert!(matches!(
        state.current_machine_state().unwrap(),
        AppStateMachine::Ready { .. }
    ));

    let streamed = collected.lock().unwrap();
    let full_text: String = streamed
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        full_text.contains("hello dictation"),
        "expected the dictation callback to receive the transcribed text, got: {full_text:?}"
    );
    drop(streamed);

    // Mirrors what the UI does after a dictation session ends: save the
    // assembled text to history.
    commands::add_dictation_entry(Arc::clone(&state), full_text.clone())
        .expect("add_dictation_entry");

    let history =
        commands::list_dictation_entries(Arc::clone(&state), None).expect("list_dictation_entries");
    assert!(
        history.iter().any(|e| e.text == full_text),
        "expected the dictation entry to be saved to history, got: {:?}",
        history.iter().map(|e| &e.text).collect::<Vec<_>>()
    );
}

/// SOU-044: the dictation shortcut's `stop_transcription` must refuse to
/// touch a meeting recording, not silently stop it (and lose the
/// accumulator) as if it were a dictation session.
#[tokio::test]
async fn stop_transcription_refuses_a_meeting_recording() {
    let mock = MockEngine::new().with_transcribe_response(
        Ok(vec![TranscriptionSegment {
            text: "hello meeting".to_string(),
            start_time: 0.0,
            end_time: 1.0,
            is_final: true,
            language: Some("en".to_string()),
            confidence: Some(0.9),
            speaker: None,
        }]),
        1,
    );
    let h = build_harness(mock);
    let state = Arc::clone(&h.state);

    let (_collected, channel) = collecting_channel();

    commands::start_meeting_recording(Arc::clone(&state), "Weekly Sync".to_string(), None, channel)
        .await
        .expect("start_meeting_recording");

    let meeting_id = match state.current_machine_state().unwrap() {
        AppStateMachine::RecordingMeeting { meeting_id, .. } => meeting_id,
        other => panic!("expected RecordingMeeting after start, got {other:?}"),
    };

    let result = commands::stop_transcription(Arc::clone(&state)).await;
    assert!(
        result.is_err(),
        "stop_transcription must refuse a meeting recording, got {result:?}"
    );

    assert!(
        matches!(
            state.current_machine_state().unwrap(),
            AppStateMachine::RecordingMeeting { .. }
        ),
        "the meeting must still be recording after the refused stop"
    );

    let accumulator_meeting_id = state
        .meeting_accumulator
        .lock()
        .unwrap()
        .as_ref()
        .map(|m| m.id.clone());
    assert_eq!(
        accumulator_meeting_id,
        Some(meeting_id),
        "meeting_accumulator must be left intact by the refused stop"
    );
}

/// Regression test for SOU-040: `peek_sleep_paused_meeting` must not clear
/// the flag on read (the old `take_sleep_paused_meeting` did, which burned
/// the id before the frontend had a chance to wait out a still-draining
/// sleep-triggered stop). The flag only goes away once a recording actually
/// starts again, via `launch_meeting`.
#[tokio::test]
async fn sleep_paused_meeting_flag_survives_peek_and_clears_on_resume() {
    let mock = MockEngine::new().with_transcribe_response(
        Ok(vec![TranscriptionSegment {
            text: "before sleep".to_string(),
            start_time: 0.0,
            end_time: 1.0,
            is_final: true,
            language: Some("en".to_string()),
            confidence: Some(0.9),
            speaker: None,
        }]),
        1,
    );
    let h = build_harness(mock);
    let state = Arc::clone(&h.state);

    let (_collected, channel) = collecting_channel();
    commands::start_meeting_recording(Arc::clone(&state), "Sleepy Sync".to_string(), None, channel)
        .await
        .expect("start_meeting_recording");

    let (session_id, meeting_id) = match state.current_machine_state().unwrap() {
        AppStateMachine::RecordingMeeting {
            session_id,
            meeting_id,
            ..
        } => (session_id, meeting_id),
        other => panic!("expected RecordingMeeting after start, got {other:?}"),
    };

    h.audio_msg_tx.send(audio_chunk(session_id)).unwrap();
    h.audio_msg_tx
        .send(AudioMessage::EndOfStream { session_id })
        .unwrap();

    // Mirror handle_system_will_sleep: remember the id, then stop through
    // the same command a user-initiated stop takes.
    state.set_sleep_paused_meeting(meeting_id.clone());
    commands::stop_meeting_recording(Arc::clone(&state))
        .await
        .expect("stop_meeting_recording");
    wait_until_ready(&state).await;

    // Two peeks in a row (matching the UI's wake handling plus a
    // belt-and-braces recheck) must both see the id: neither call may
    // consume it.
    assert_eq!(
        commands::peek_sleep_paused_meeting(Arc::clone(&state)),
        Some(meeting_id.clone())
    );
    assert_eq!(
        commands::peek_sleep_paused_meeting(Arc::clone(&state)),
        Some(meeting_id.clone())
    );

    // Resuming goes through launch_meeting, which clears the flag
    // unconditionally: a later wake must not re-offer this meeting.
    let (_collected2, channel2) = collecting_channel();
    commands::resume_meeting_recording(Arc::clone(&state), meeting_id.clone(), channel2)
        .await
        .expect("resume_meeting_recording");

    assert_eq!(
        commands::peek_sleep_paused_meeting(Arc::clone(&state)),
        None
    );
}

/// `clear_sleep_paused_meeting` lets the frontend drop the flag on an
/// explicit user refusal (or once a resume actually starts) without going
/// through a full recording start.
#[tokio::test]
async fn clear_sleep_paused_meeting_command_clears_without_resuming() {
    let h = build_harness(MockEngine::new());
    let state = Arc::clone(&h.state);

    state.set_sleep_paused_meeting("meeting-x".to_string());
    assert_eq!(
        commands::peek_sleep_paused_meeting(Arc::clone(&state)),
        Some("meeting-x".to_string())
    );

    commands::clear_sleep_paused_meeting(Arc::clone(&state));

    assert_eq!(
        commands::peek_sleep_paused_meeting(Arc::clone(&state)),
        None
    );
}

#[tokio::test]
async fn failed_resume_preserves_sleep_paused_flag() {
    let h = build_harness(MockEngine::new());
    let state = Arc::clone(&h.state);

    // Mock a sleeping meeting
    state.set_sleep_paused_meeting("meeting-x".to_string());
    assert_eq!(
        commands::peek_sleep_paused_meeting(Arc::clone(&state)),
        Some("meeting-x".to_string())
    );

    // Drop the audio receiver so the command channel send in start_pipeline_blocking fails
    drop(h.audio_cmd_rx);

    let (_collected, channel) = collecting_channel();
    let result =
        commands::resume_meeting_recording(Arc::clone(&state), "meeting-x".to_string(), channel)
            .await;
    assert!(
        result.is_err(),
        "resume should fail because audio cmd channel is dropped"
    );

    // The flag must survive because the resume failed
    assert_eq!(
        commands::peek_sleep_paused_meeting(Arc::clone(&state)),
        Some("meeting-x".to_string())
    );
}
