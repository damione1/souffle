//! SOU-151 visual fixture: real MainWindow, policy and delete commands, temp DB
//! only. Recording/Stopping are simulated; no microphone, TCC or user profile.
//! GUARD_SCENE=detail selects the finalizing detail; otherwise the timeline.
//! GUARD_LOCALE selects a bundled translation (for example fr or en).
use souffle_slint::*;
#[allow(dead_code)]
#[path = "../src/action_guards.rs"]
mod action_guards;
#[allow(dead_code)]
#[path = "../src/timeline.rs"]
mod timeline;
#[allow(dead_code)]
#[path = "../src/typography.rs"]
mod typography;

use slint::ComponentHandle;
use souffle_lib::engine::default_transcription_profile;
use souffle_lib::meeting_access::MeetingDeleteError;
use souffle_lib::state::AppState;
use souffle_lib::state_machine::{AppStateMachine, RecordingKind};
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let db = Arc::new(souffle_lib::db::Database::open(
        &dir.path().join("fixture.db"),
    )?);
    let now = chrono::Utc::now();
    for (id, title) in [
        ("active", "Réunion protégée (fixture SOU-151)"),
        ("old", "Réunion archivée supprimable (fixture)"),
    ] {
        db.save_meeting(&souffle_lib::transcript::MeetingTranscript {
            id: id.into(),
            title: title.into(),
            started_at: now,
            ended_at: Some(now),
            duration_seconds: 60.0,
            transcription_profile: default_transcription_profile(),
            recording_sessions: vec![],
            segments: vec![],
            summary: None,
            summary_is_stale: false,
            summary_model: None,
            summary_generated_at: None,
            structured_summary: None,
            edited_transcript: None,
            notes: None,
            calendar_event_id: None,
            participants: vec![],
            system_audio: None,
        })?;
    }
    let (_audio, rx) = crossbeam_channel::unbounded();
    let actor = souffle_lib::pipeline::EngineActorHandle::spawn(
        rx,
        Arc::new(std::sync::atomic::AtomicU64::new(0)),
        Arc::new(std::sync::Mutex::new(None)),
        Box::new(|_| Err("Visual fixture has no engine".into())),
    )?;
    let (tx, _commands) = crossbeam_channel::unbounded();
    let mut state = AppState::new(
        tx,
        Arc::new(actor),
        db,
        Arc::new(std::sync::atomic::AtomicU32::new(0)),
    );
    state.recordings_root = dir.path().join("recordings");
    let state = Arc::new(state);
    let detail = std::env::var("GUARD_SCENE").as_deref() == Ok("detail");
    *state.machine.lock().unwrap() = if detail {
        AppStateMachine::Stopping {
            profile: default_transcription_profile(),
            was_recording: RecordingKind::Meeting {
                meeting_id: "active".into(),
            },
        }
    } else {
        AppStateMachine::RecordingMeeting {
            profile: default_transcription_profile(),
            session_id: 1,
            meeting_id: "active".into(),
        }
    };
    slint::BackendSelector::new()
        .renderer_name("skia".into())
        .select()?;
    let window = MainWindow::new()?;
    typography::initialize(&window)?;
    // Creating the component registers its bundled translations first.
    if let Ok(locale) = std::env::var("GUARD_LOCALE") {
        slint::select_bundled_translation(&locale)?;
    }
    window
        .window()
        .set_size(slint::LogicalSize::new(860.0, 780.0));
    action_guards::install(&window, &state);
    let (events_tx, events_rx) = crossbeam_channel::unbounded();
    souffle_lib::native::bridge::set_sink(events_tx);
    let weak = window.as_weak();
    std::thread::spawn(move || {
        use souffle_lib::native::bridge::NativeAction;
        for action in events_rx {
            match action {
                NativeAction::MeetingDeletePermissionsChanged | NativeAction::RefreshRuntime => {
                    let weak = weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(window) = weak.upgrade() {
                            action_guards::refresh(&window);
                        }
                    });
                }
                NativeAction::ToggleDictation
                | NativeAction::PttStart
                | NativeAction::PttStop
                | NativeAction::StopMeeting
                | NativeAction::StopDictation
                | NativeAction::CancelDictation
                | NativeAction::Navigate(_)
                | NativeAction::ShowMainWindow
                | NativeAction::Quit
                | NativeAction::QuitFailed(_)
                | NativeAction::MicStallChanged
                | NativeAction::SystemWokeUp
                | NativeAction::UpdateAvailable { .. } => {}
            }
        }
    });
    window.set_onboarding_open(false);
    window.set_settings_calendar_enabled(false);
    window.set_live_meeting_id("active".into());
    window.set_timeline_is_empty(false);
    window.set_timeline_has_matches(true);
    refresh(&window, &state);
    if detail {
        window.set_active_meeting_id("active".into());
        window.set_meeting_detail_title("Réunion en finalisation (fixture SOU-151)".into());
        window.set_meeting_detail_notes("Données temporaires. Aucun enregistrement réel.".into());
    }
    let weak = window.as_weak();
    let handle = Arc::clone(&state);
    window.on_timeline_item_removed(move |_, id| {
        match action_guards::delete_from_handler(&handle, &id) {
            Ok(()) => {
                eprintln!("Fixture deleted archived meeting {id}");
                if let Some(window) = weak.upgrade() {
                    refresh(&window, &handle);
                }
            }
            Err(MeetingDeleteError::Blocked(reason)) => {
                eprintln!("Fixture refused deletion: {reason:?}")
            }
            Err(MeetingDeleteError::Failed(error)) => eprintln!("Fixture error: {error}"),
        }
    });
    window.on_meeting_detail_delete(move || {
        assert!(matches!(
            action_guards::delete_from_handler(&state, "active"),
            Err(MeetingDeleteError::Blocked(_))
        ));
    });
    window.run()?;
    Ok(())
}

fn refresh(window: &MainWindow, state: &AppState) {
    let meetings = state.db.list_meetings().unwrap();
    window.set_timeline_groups(slint::ModelRc::new(slint::VecModel::from(
        timeline::build_groups(&[], &meetings, TimelineFilter::All, ""),
    )));
}
