//! Weight-backed native live-view QA for SOU-273, without touching user data
//! or microphone permissions. Feed SOU273_PCM (16 kHz mono JSON samples from
//! batch_preview_replay), choose SOU273_ENGINE and optionally SOU273_DICTATION=1.
//! Run from app/ so the bundled VAD/ORT resources resolve.

slint::include_modules!();
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

use souffle_lib::audio::{AudioChunk, AudioMessage};
use souffle_lib::filter::{PipelineConfig, resolve_vad_model_path};
use souffle_lib::pipeline::{EngineActorHandle, SessionConfig};
use std::sync::{Arc, Mutex, atomic::AtomicU64};
use std::time::{Duration, Instant};

fn main() {
    slint::BackendSelector::new()
        .renderer_name("skia".into())
        .select()
        .unwrap();
    let window = MainWindow::new().unwrap();
    window
        .window()
        .set_size(slint::LogicalSize::new(1040.0, 820.0));
    window.set_onboarding_open(false);
    window.set_recording_mode(if std::env::var("SOU273_DICTATION").as_deref() == Ok("1") {
        RecordingMode::Dictation
    } else {
        RecordingMode::Meeting
    });
    let live = Arc::new(Mutex::new(live_transcript::LiveTranscript::new()));
    let generation = live.lock().unwrap().generation();
    let weak = window.as_weak();
    let pcm: Vec<f32> = serde_json::from_slice(
        &std::fs::read(std::env::var("SOU273_PCM").expect("set SOU273_PCM")).unwrap(),
    )
    .unwrap();
    let engine = std::env::var("SOU273_ENGINE").expect("set SOU273_ENGINE");
    let profile =
        souffle_lib::engine::resolve_transcription_profile(Some(&engine), None, None).unwrap();
    let (tx, rx) = crossbeam_channel::bounded(200);
    let actor = EngineActorHandle::spawn(
        rx,
        Arc::new(AtomicU64::new(0)),
        Arc::new(Mutex::new(None)),
        Box::new(souffle_lib::engine::create_engine),
    )
    .unwrap();
    let worker = std::thread::spawn(move || {
        actor
            .load_model(profile.clone(), souffle_lib::models::model_dir(&profile))
            .unwrap();
        let started = Arc::new(Mutex::new(Instant::now()));
        let callback_started = started.clone();
        let callback_live = live.clone();
        let callback_weak = weak.clone();
        actor.start_session(273, SessionConfig {
            pipeline_config: PipelineConfig { vad_enabled: true, vad_model_path: Some(resolve_vad_model_path().unwrap()), filler_removal_enabled: true, stutter_collapse_enabled: true, dictionary_correction_enabled: false },
            dictionary_entries: vec![], session_terms: vec![], session_corrections: vec![], diarize: false,
            idle_config: None, meeting_transcription_language: souffle_lib::settings::MeetingTranscriptionLanguage::Auto,
        }, Box::new(move |segment| {
            let published = Instant::now();
            let elapsed = callback_started.lock().unwrap().elapsed().as_secs_f64();
            let live = callback_live.clone();
            let _ = callback_weak.upgrade_in_event_loop(move |window| {
                eprintln!("UI segment final={} actor_at={elapsed:.3}s slint_queue={:.3}s text={:?}", segment.is_final, published.elapsed().as_secs_f64(), segment.text);
                live_view::apply_live_segment_for_generation(&window, &live, generation, &segment);
            });
        })).unwrap();
        *started.lock().unwrap() = Instant::now();
        let start = Instant::now();
        for (i, chunk) in pcm.chunks(1600).enumerate() {
            std::thread::sleep(
                (start + Duration::from_millis((i as u64 + 1) * 100))
                    .saturating_duration_since(Instant::now()),
            );
            tx.send(AudioMessage::Chunk(AudioChunk {
                queue_permit: None,
                session_id: 273,
                samples: chunk.to_vec(),
                captured_at: Instant::now(),
                speaker: None,
            }))
            .unwrap();
        }
        tx.send(AudioMessage::EndOfStream { session_id: 273 })
            .unwrap();
        let summary = actor.stop_session(Duration::from_secs(60)).unwrap();
        eprintln!("Stopped: {summary:?}");
        actor.shutdown().unwrap();
        let _ = weak.upgrade_in_event_loop(move |window| {
            live.lock().unwrap().clear();
            live_view::push_live_blocks(&window, &live);
            window.set_live_tentative("".into());
            live_view::push_dictation_words(&window);
        });
        std::thread::sleep(Duration::from_secs(2));
        slint::quit_event_loop().unwrap();
    });
    window.run().unwrap();
    worker.join().unwrap();
}
