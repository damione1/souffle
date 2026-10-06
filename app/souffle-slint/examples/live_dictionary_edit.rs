//! Isolated native QA for SOU-166. The real Slint editor and suggestion
//! handlers call production commands against a temporary SQLite database.
//! No user profile, audio device or permission is touched. Run with
//! `cargo run -p souffle-slint --example live_dictionary_edit`; use
//! SOU166_LOCALE=en for the English surface. Correct Kubernetis to Kubernetes,
//! then open Settings / Transcription to accept or dismiss the suggestion.

slint::include_modules!();
#[allow(dead_code)]
#[path = "../src/lists_ui.rs"]
mod lists_ui;
#[allow(dead_code)]
#[path = "../src/live_edit.rs"]
mod live_edit;
#[allow(dead_code)]
#[path = "../src/live_transcript.rs"]
mod live_transcript;
#[allow(dead_code)]
#[path = "../src/live_view.rs"]
mod live_view;
#[allow(dead_code)]
#[path = "../src/timeline.rs"]
mod timeline;
#[allow(dead_code)]
#[path = "../src/transcript.rs"]
mod transcript;

use slint::ComponentHandle;
use souffle_lib::{
    commands,
    db::Database,
    engine::{Speaker, TranscriptionSegment},
    settings::{AppSettings, DictionaryLearningMode as LearningMode},
    state::{AppState, MeetingAccumulator},
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, AtomicU64},
};

fn main() {
    slint::BackendSelector::new()
        .renderer_name("skia".into())
        .select()
        .unwrap();
    let locale = std::env::var("SOU166_LOCALE").unwrap_or_else(|_| "fr".into());
    let temp = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(&temp.path().join("qa.db")).unwrap());
    AppSettings {
        dictionary_learning_mode: LearningMode::Suggestions,
        ..AppSettings::default()
    }
    .save(&db)
    .unwrap();
    let (_audio_tx, audio_rx) = crossbeam_channel::unbounded();
    let actor = souffle_lib::pipeline::EngineActorHandle::spawn(
        audio_rx,
        Arc::new(AtomicU64::new(0)),
        Arc::new(Mutex::new(None)),
        Box::new(|_| Ok(Box::new(souffle_lib::engine::mock::MockEngine::new()))),
    )
    .unwrap();
    let (command_tx, _command_rx) = crossbeam_channel::unbounded();
    let state = Arc::new(AppState::new(
        command_tx,
        Arc::new(actor),
        db.clone(),
        Arc::new(AtomicU32::new(0)),
    ));
    let window = MainWindow::new().unwrap();
    slint::select_bundled_translation(&locale).unwrap();
    window
        .window()
        .set_size(slint::LogicalSize::new(1040.0, 820.0));
    window.set_onboarding_open(false);
    window.set_recording_mode(RecordingMode::Meeting);
    window.set_live_meeting_id("dictionary-qa".into());
    let live = Arc::new(Mutex::new(live_transcript::LiveTranscript::new()));
    let segments = vec![TranscriptionSegment {
        text: "Nous utilisons Kubernetis pour le projet.".into(),
        start_time: 3.0,
        end_time: 6.0,
        is_final: true,
        speaker: Some(Speaker::Me),
        language: Some("fr".into()),
        confidence: None,
    }];
    *state.meeting_accumulator.lock().unwrap() = Some(MeetingAccumulator {
        id: "dictionary-qa".into(),
        title: "Dictionary QA".into(),
        existing_segments: vec![],
        new_segments: segments.clone(),
        recording_sessions: vec![],
        session_started_at: chrono::Utc::now(),
        transcription_profile: souffle_lib::engine::default_transcription_profile(),
        summary: None,
        summary_is_stale: false,
        summary_model: None,
        summary_generated_at: None,
        structured_summary: None,
        notes: None,
        calendar_event_id: None,
        participants: vec![],
        persisted_new_count: 0,
    });
    for segment in &segments {
        live_view::apply_live_segment(&window, &live, segment);
    }
    live_edit::wire_callbacks(&window, state.clone(), live);
    let models = lists_ui::SettingsListModels::install(&window);
    window.set_settings_dictionary_learning_mode(DictionaryLearningMode::Suggestions);
    window.set_settings_dictionary_learning_options(slint::ModelRc::new(slint::VecModel::from(
        LearningMode::ALL
            .into_iter()
            .map(|mode| DictionaryLearningOption {
                mode: match mode {
                    LearningMode::Disabled => DictionaryLearningMode::Disabled,
                    LearningMode::Suggestions => DictionaryLearningMode::Suggestions,
                    LearningMode::Automatic => DictionaryLearningMode::Automatic,
                },
            })
            .collect::<Vec<_>>(),
    )));
    let weak = window.as_weak();
    let read_state = state.clone();
    let read_models = models.clone();
    window.on_settings_requested(move || {
        let window = weak.upgrade().unwrap();
        read_models.populate_dictionary(&commands::list_dictionary(read_state.clone()).unwrap());
        read_models.populate_suggestions(
            &commands::list_dictionary_suggestions(read_state.clone()).unwrap(),
        );
        window.set_settings_tab(SettingsTab::Transcription);
        window.set_settings_open(true);
    });
    let weak = window.as_weak();
    window.on_settings_closed(move || weak.upgrade().unwrap().set_settings_open(false));
    let weak = window.as_weak();
    let accept_state = state.clone();
    let accept_models = models.clone();
    window.on_settings_dictionary_suggestion_accept_requested(move |id| {
        lists_ui::mutate_dictionary_suggestion(
            weak.clone(),
            accept_state.clone(),
            accept_models.clone(),
            id.into(),
            true,
        );
    });
    let weak = window.as_weak();
    window.on_settings_dictionary_suggestion_dismiss_requested(move |id| {
        lists_ui::mutate_dictionary_suggestion(
            weak.clone(),
            state.clone(),
            models.clone(),
            id.into(),
            false,
        );
    });
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(2),
        move || {
            eprintln!(
                "SOU166 QA pending={:?} accepted={:?}",
                db.list_dictionary_suggestions().unwrap(),
                db.list_dictionary_entries().unwrap()
            );
        },
    );
    window.run().unwrap();
}
