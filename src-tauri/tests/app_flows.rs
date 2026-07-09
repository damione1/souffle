//! Smoke-level end-to-end tests for the two critical lifecycles: dictation
//! round-trip and "meeting stop persists a meeting". Drives the real Tauri
//! command functions from `souffle_lib::commands` against a
//! `tauri::test::MockRuntime` app, with a `MockEngine` swapped in for the
//! transcription engine (via the actor's injectable `EngineFactory`) and a
//! temp-file SQLite database. No GPU, no real audio hardware, no webview —
//! `tauri-driver` does not support macOS, so this exercises the command
//! layer directly instead of driving a real window through WebDriver.
//!
//! ## Coverage note: why `stop_meeting_recording` isn't called directly
//!
//! `AppState::app_handle` is a concrete `tauri::AppHandle` — the
//! `#[default_runtime(crate::Wry, wry)]` macro Tauri applies to `AppHandle`
//! makes the bare (unparameterized) name mean `AppHandle<Wry>`, not generic
//! over the runtime. A `MockRuntime`-backed app can never produce one (that
//! would be a type error), so `state.app_handle` stays `None` for this whole
//! suite — exactly what `AppState::apply_transition` already tolerates (its
//! event-emission block is a no-op when `app_handle` is unset, clearly
//! designed with this in mind).
//!
//! `start_meeting_recording`, `resume_meeting_recording`, `start_transcription`,
//! `stop_transcription`, and every read-path command never touch
//! `app_handle`, so those run as the literal, unmodified `#[tauri::command]`
//! functions below. `stop_meeting_recording` is the one exception: it calls
//! `state.app_handle()` to spawn a background finalize task that looks
//! itself up again via `AppHandle::state()` and emits `MeetingFinalized` for
//! the frontend. Building a real, window-backed `AppHandle<Wry>` needs the
//! platform event loop on the main thread, which conflicts with the
//! `cargo test` harness's threading model — the same class of fragility as
//! the tauri-driver-on-macOS gap this test suite exists to work around. So
//! `stop_meeting_and_persist` below drives the exact same primitives that
//! background task uses (`EngineActorHandle::stop_session`,
//! `MeetingAccumulator::into_transcript`, `Database::save_meeting`, the
//! `StopRecording`/`StopComplete` transitions) directly, synchronously,
//! instead of through the outer AppHandle-spawning wrapper. Everything
//! except that background-task glue itself is exercised.

use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use tauri::Manager;
use tauri::ipc::{Channel, InvokeResponseBody};
use tempfile::TempDir;

use souffle_lib::audio::{AudioChunk, AudioMessage};
use souffle_lib::audio::system_activity::SystemAudioActivity;
use souffle_lib::commands;
use souffle_lib::constants::MIMI_FRAME_SIZE;
use souffle_lib::db::Database;
use souffle_lib::engine::mock::MockEngine;
use souffle_lib::engine::{TranscriptionEngine, TranscriptionSegment, default_transcription_profile};
use souffle_lib::pipeline::EngineActorHandle;
use souffle_lib::settings::AppSettings;
use souffle_lib::state::{AppState, AudioCommand};
use souffle_lib::state_machine::{AppStateMachine, StateAction};

/// Spawn the engine actor with a `MockEngine` in place of the real
/// transcription backend, mirroring `pipeline::actor::tests::spawn_with_mock`.
fn spawn_mock_actor(mock: MockEngine) -> (EngineActorHandle, Sender<AudioMessage>) {
    let (audio_tx, audio_rx) = crossbeam_channel::unbounded();
    let cell = Mutex::new(Some(mock));
    let actor = EngineActorHandle::spawn(
        audio_rx,
        Arc::new(std::sync::atomic::AtomicU64::new(0)),
        Arc::new(Mutex::new(None)),
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
    state.apply_transition(StateAction::StartLoad).unwrap();
    state.apply_transition(StateAction::LoadComplete).unwrap();
}

/// Test harness: everything needed to call commands end-to-end plus the
/// pieces (audio channel, actor, db) needed to drive/inspect them directly.
struct Harness {
    app: tauri::App<tauri::test::MockRuntime>,
    db: Arc<Database>,
    actor: Arc<EngineActorHandle>,
    audio_msg_tx: Sender<AudioMessage>,
    /// Kept alive so `AppState::audio_cmd_sender.send(...)` never errors with
    /// "all receivers dropped" — nothing needs to read from it since there is
    /// no real audio-capture thread in this test.
    _audio_cmd_rx: Receiver<AudioCommand>,
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
    let system_audio_activity = Arc::new(SystemAudioActivity::default());

    let app_state = AppState::new(
        audio_cmd_tx,
        Arc::clone(&actor),
        Arc::clone(&db),
        audio_rms,
        system_audio_activity,
    );
    bring_to_ready(&app_state, &actor);

    let app = tauri::test::mock_builder()
        .manage(app_state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build mock tauri app");

    Harness {
        app,
        db,
        actor,
        audio_msg_tx,
        _audio_cmd_rx: audio_cmd_rx,
        _tmp: tmp,
    }
}

/// A non-silent chunk carrying exactly one engine frame's worth of samples,
/// tagged for the given session (matches `pipeline::actor::tests::audio_chunk`).
fn audio_chunk(session_id: u64) -> AudioMessage {
    AudioMessage::Chunk(AudioChunk {
        session_id,
        samples: vec![0.1f32; MIMI_FRAME_SIZE],
        captured_at: std::time::Instant::now(),
        speaker: None,
    })
}

/// A `Channel` that deserializes every message as `TranscriptionSegment` and
/// appends it to a shared `Vec`, so tests can assert on what streamed back.
fn collecting_channel() -> (
    Arc<Mutex<Vec<TranscriptionSegment>>>,
    Channel<TranscriptionSegment>,
) {
    let collected: Arc<Mutex<Vec<TranscriptionSegment>>> = Arc::new(Mutex::new(Vec::new()));
    let collected_ref = Arc::clone(&collected);
    let channel = Channel::new(move |body| {
        if let InvokeResponseBody::Json(json) = body
            && let Ok(segment) = serde_json::from_str::<TranscriptionSegment>(&json)
        {
            collected_ref.lock().unwrap().push(segment);
        }
        Ok(())
    });
    (collected, channel)
}

/// Stop an active meeting recording and persist it, driving the same
/// primitives `commands::stop_meeting_recording`'s background task uses (see
/// the module doc for why the command itself isn't called here).
fn stop_meeting_and_persist(h: &Harness, state: &tauri::State<'_, AppState>) {
    assert!(
        state.current_machine_state().unwrap().is_recording(),
        "expected an active recording session before stopping"
    );
    state
        .apply_transition(StateAction::StopRecording)
        .expect("StopRecording transition");

    let _ = state.audio_cmd_sender.send(AudioCommand::Stop);
    h.actor
        .stop_session(Duration::from_secs(5))
        .expect("actor stop_session");

    if let Ok(mut guard) = state.meeting_accumulator.lock()
        && let Some(meeting) = guard.take()
    {
        let transcript = meeting.into_transcript(chrono::Utc::now());
        h.db.save_meeting(&transcript).expect("save meeting");
    }

    state
        .apply_transition(StateAction::StopComplete)
        .expect("StopComplete transition");
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
    let state = h.app.state::<AppState>();

    let (collected, channel) = collecting_channel();

    commands::start_meeting_recording(state.clone(), "Weekly Sync".to_string(), None, channel)
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

    stop_meeting_and_persist(&h, &state);

    assert!(matches!(
        state.current_machine_state().unwrap(),
        AppStateMachine::Ready { .. }
    ));

    // Segments streamed live to the frontend during the meeting.
    let streamed = collected.lock().unwrap();
    assert!(
        streamed.iter().any(|s| s.text == "hello meeting"),
        "expected a live-streamed 'hello meeting' segment, got: {:?}",
        streamed.iter().map(|s| &s.text).collect::<Vec<_>>()
    );
    drop(streamed);

    // The meeting row exists with ended_at set and the segment persisted —
    // the actual "stop persists a meeting" assertion.
    let meeting = h.db.load_meeting(&meeting_id).expect("load persisted meeting");
    assert_eq!(meeting.title, "Weekly Sync");
    assert!(meeting.ended_at.is_some(), "meeting should be finalized");
    assert!(
        meeting.segments.iter().any(|s| s.text == "hello meeting"),
        "expected the transcribed segment to be persisted, got: {:?}",
        meeting.segments.iter().map(|s| &s.text).collect::<Vec<_>>()
    );

    let list = commands::list_meetings(state.clone()).expect("list_meetings");
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
    let state = h.app.state::<AppState>();

    let (collected, channel) = collecting_channel();

    commands::start_transcription(state.clone(), channel)
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

    // `stop_transcription` never touches `AppState::app_handle`, so — unlike
    // the meeting stop above — it runs as the real, unmodified command.
    commands::stop_transcription(state.clone())
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

    // Mirrors what the frontend does after a dictation session ends: save
    // the assembled text to history.
    commands::add_dictation_entry(state.clone(), full_text.clone())
        .expect("add_dictation_entry");

    let history = commands::list_dictation_entries(state.clone(), None)
        .expect("list_dictation_entries");
    assert!(
        history.iter().any(|e| e.text == full_text),
        "expected the dictation entry to be saved to history, got: {:?}",
        history.iter().map(|e| &e.text).collect::<Vec<_>>()
    );
}
