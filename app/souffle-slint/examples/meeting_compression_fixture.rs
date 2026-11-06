//! Synthetic native compression QA. No bootstrap, microphone, TCC or personal DB.
//! COMPRESSION_FIXTURE_CACHE points to the checksum-verified prepared model cache.
use slint::ComponentHandle;
use souffle_lib::{state::AppState, transcript::MeetingTranscript};
use souffle_slint::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, AtomicU64},
};
#[path = "../src/typography.rs"]
mod typography;
fn show(window: &MainWindow, state: &AppState, id: &str) {
    let meeting = state.db.load_meeting(id).unwrap();
    window.set_active_meeting_id(id.into());
    window.set_meeting_detail_title(meeting.title.clone().into());
    window.set_meeting_detail_notes(meeting.notes.clone().unwrap_or_default().into());
    window.set_meeting_detail_transcript_segment_count(meeting.segments.len() as i32);
    window.set_meeting_detail_can_resume(false);
    compression_ui::populate(window, &meeting);
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = std::env::var_os("COMPRESSION_FIXTURE_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| dir.path().join("fixture.db"));
    let db = Arc::new(souffle_lib::db::Database::open(&path)?);
    let now = chrono::Utc::now();
    for (id, title) in [
        ("compression-fixture", "Compression · réunion synthétique"),
        ("second-fixture", "Navigation · deuxième réunion"),
    ] {
        if !db.meeting_exists(id)? {
            let meeting=MeetingTranscript{id:id.into(),title:title.into(),started_at:now,ended_at:Some(now),duration_seconds:180.,transcription_profile:Default::default(),recording_sessions:vec![],segments:vec![souffle_lib::engine::TranscriptionSegment{text:"Élodie Martin ne paie pas 42 euros. Nous allons revoir ensemble tous les détails du projet et la liste des choses possibles avant la décision finale.".into(),start_time:12.,end_time:45.,is_final:true,language:Some("fr".into()),confidence:None,speaker:Some(souffle_lib::engine::Speaker::Me)},souffle_lib::engine::TranscriptionSegment{text:"The team discussed the planned release date and several possible follow-up activities, but no additional commitment was made during the meeting.".into(),start_time:60.,end_time:90.,is_final:true,language:Some("en".into()),confidence:None,speaker:Some(souffle_lib::engine::Speaker::Them)}],summary:None,compact_transcript:None,summary_is_stale:false,summary_model:None,summary_generated_at:None,structured_summary:None,edited_transcript:None,notes:Some("Synthetic fixture only".into()),calendar_event_id:None,participants:vec![souffle_lib::transcript::MeetingParticipant{name:"Élodie Martin".into(),email:None,is_organizer:false,is_current_user:false}],system_audio:None};
            db.save_meeting(&meeting)?;
        }
    }
    let (_audio, rx) = crossbeam_channel::unbounded();
    let actor = souffle_lib::pipeline::EngineActorHandle::spawn(
        rx,
        Arc::new(AtomicU64::new(0)),
        Arc::new(Mutex::new(None)),
        Box::new(|_| Err("No capture in compression fixture".into())),
    )?;
    let (tx, _commands) = crossbeam_channel::unbounded();
    let mut state = AppState::new(tx, Arc::new(actor), db.clone(), Arc::new(AtomicU32::new(0)));
    state.recordings_root = dir.path().join("recordings");
    if let Some(path) = std::env::var_os("COMPRESSION_FIXTURE_CACHE") {
        state.compression = Arc::new(souffle_lib::summary::compress::CompressionService::new(
            path.into(),
        ));
    }
    let state = Arc::new(state);
    eprintln!("Synthetic DB: {}", path.display());
    slint::BackendSelector::new()
        .renderer_name("skia".into())
        .select()?;
    let window = MainWindow::new()?;
    typography::initialize(&window)?;
    slint::select_bundled_translation(
        &std::env::var("COMPRESSION_FIXTURE_LOCALE").unwrap_or_else(|_| "en".into()),
    )?;
    window
        .window()
        .set_size(slint::LogicalSize::new(1040., 900.));
    let timer = compression_ui::install(&window, state.clone());
    compression_ui::populate_settings(
        &window,
        &souffle_lib::settings::AppSettings::load_read_only(&db)?,
    );
    show(&window, &state, "compression-fixture");
    // Same persisted command + after-commit installer used by the production settings coordinator.
    for kind in 0..3 {
        let state = state.clone();
        let weak = window.as_weak();
        let callback = move |value: i32| {
            let state = state.clone();
            let weak = weak.clone();
            souffle_lib::async_runtime::spawn(async move {
                let mut settings =
                    souffle_lib::settings::AppSettings::load_read_only(&state.db).unwrap();
                match kind {
                    0 => settings.summary_compression_enabled = value != 0,
                    1 => settings.compact_transcript_enabled = value != 0,
                    2 => settings.summary_compression_rate = value as f32 / 100.,
                    _ => unreachable!(),
                }
                let outcome = souffle_lib::commands::save_settings_observed_in_lane(
                    state,
                    settings,
                    souffle_lib::commands::SettingsSaveLane::General,
                );
                if let souffle_lib::commands::SettingsSaveOutcome::Observed { settings, .. } =
                    outcome
                {
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        compression_ui::populate_settings(&w, &settings)
                    });
                }
            });
        };
        match kind {
            0 => window
                .global::<CompressionUi>()
                .on_summary_changed(move |enabled| callback(i32::from(enabled))),
            1 => window
                .global::<CompressionUi>()
                .on_alongside_changed(move |enabled| callback(i32::from(enabled))),
            2 => window.global::<CompressionUi>().on_rate_changed(callback),
            _ => unreachable!(),
        }
    }
    let weak = window.as_weak();
    let navigation = state.clone();
    window.on_meeting_detail_back(move || {
        if let Some(w) = weak.upgrade() {
            let next = if w.get_active_meeting_id().as_str() == "compression-fixture" {
                "second-fixture"
            } else {
                "compression-fixture"
            };
            show(&w, &navigation, next);
        }
    });
    let weak = window.as_weak();
    window.on_meeting_detail_rename(move |title| {
        if let Some(w) = weak.upgrade() {
            state
                .db
                .save_edited_transcript(w.get_active_meeting_id().as_str(), Some(&title))
                .unwrap();
            show(&w, &state, w.get_active_meeting_id().as_str());
        }
    });
    let weak = window.as_weak();
    let settings_db = db.clone();
    window.on_settings_requested(move || {
        if let Some(w) = weak.upgrade() {
            if let Ok(settings) = souffle_lib::settings::AppSettings::load_read_only(&settings_db) {
                compression_ui::populate_settings(&w, &settings);
            }
            w.set_settings_open(true);
        }
    });
    window.set_settings_tab(SettingsTab::Ai);
    if std::env::var("COMPRESSION_FIXTURE_SCENE").as_deref() == Ok("settings") {
        window.set_settings_open(true);
    }
    window.run()?;
    drop(timer);
    Ok(())
}
