//! Weight-backed native live-view QA for SOU-273, without touching user data
//! or microphone permissions. Feed SOU273_PCM (16 kHz mono JSON samples from
//! batch_preview_replay), choose SOU273_ENGINE and optionally SOU273_DICTATION=1.
//! SOU275_PCM_RATE overrides the input fixture rate; the engine contract
//! determines resampling, delivery hop and pacing. SOU275_HOLD_SECONDS keeps
//! the finished fixture transcript visible for native screenshot inspection.
//! SOUFFLE_REPLAY_INSTALL_ASSETS=1 explicitly installs/reserves the chosen
//! profile's assets (needed for system Speech in this separate test process).
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

use slint::Model;
use souffle_lib::audio::{AudioChunk, AudioMessage, Resampler};
use souffle_lib::filter::{PipelineConfig, resolve_vad_model_path};
use souffle_lib::pipeline::{EngineActorHandle, SessionConfig};
use std::sync::{Arc, Mutex, atomic::AtomicU64};
use std::time::{Duration, Instant};

fn resample_fixture(pcm: Vec<f32>, source_rate: u32, target_rate: u32) -> Vec<f32> {
    let samples = (pcm.len() as u64 * u64::from(target_rate) / u64::from(source_rate)) as usize;
    let mut resampler = Resampler::new(source_rate, 1, target_rate, 1.0);
    let mut output = resampler.process(&pcm);
    output.extend(resampler.flush());
    // The production resampler pads its last FFT frame. The fixture's known
    // duration supplies the exact common clock for both replayed sources.
    output.resize(samples, 0.0);
    output
}

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
    let system_pcm: Option<Vec<f32>> = std::env::var("SOU274_SYSTEM_PCM")
        .ok()
        .map(|path| serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap());
    if let Some(system) = &system_pcm {
        assert_eq!(system.len(), pcm.len());
    }
    let dual = system_pcm.is_some();
    let engine = std::env::var("SOU273_ENGINE").expect("set SOU273_ENGINE");
    let profile =
        souffle_lib::engine::resolve_transcription_profile(Some(&engine), None, None).unwrap();
    let source_rate: u32 = std::env::var("SOU275_PCM_RATE")
        .unwrap_or_else(|_| "16000".into())
        .parse()
        .unwrap();
    let hold_seconds: u64 = std::env::var("SOU275_HOLD_SECONDS")
        .unwrap_or_else(|_| "2".into())
        .parse()
        .unwrap();
    let (tx, rx) = crossbeam_channel::bounded(200);
    let actor = EngineActorHandle::spawn(
        rx,
        Arc::new(AtomicU64::new(0)),
        Arc::new(Mutex::new(None)),
        Box::new(souffle_lib::engine::create_engine),
    )
    .unwrap();
    let worker = std::thread::spawn(move || {
        if std::env::var("SOUFFLE_REPLAY_INSTALL_ASSETS").as_deref() == Ok("1") {
            souffle_lib::models::download_model(&profile, |_| {}).unwrap();
        }
        let info = actor
            .load_model(
                profile.clone(),
                souffle_lib::models::model_location(&profile).unwrap(),
            )
            .unwrap();
        // System-managed analyzers negotiate their PCM format at load time.
        // The loaded contract drives replay for every engine.
        let audio_input = info.audio;
        assert!(source_rate > 0 && audio_input.sample_rate_hz > 0);
        let hop = audio_input.chunk_size_samples as usize;
        assert!(hop > 0);
        let pcm = resample_fixture(pcm, source_rate, audio_input.sample_rate_hz);
        let system_pcm = system_pcm
            .map(|system| resample_fixture(system, source_rate, audio_input.sample_rate_hz));
        eprintln!(
            "Fixture audio source_hz={source_rate} engine_hz={} hop={hop} samples={} duration={:.3}s dual={dual}",
            audio_input.sample_rate_hz,
            pcm.len(),
            pcm.len() as f64 / f64::from(audio_input.sample_rate_hz)
        );
        let started = Arc::new(Mutex::new(Instant::now()));
        let callback_started = started.clone();
        let callback_live = live.clone();
        let callback_weak = weak.clone();
        actor.start_session(273, SessionConfig {
            pipeline_config: PipelineConfig { vad_enabled: true, vad_model_path: Some(resolve_vad_model_path().unwrap()), filler_removal_enabled: true, stutter_collapse_enabled: true, dictionary_correction_enabled: false },
            dictionary_entries: vec![], session_terms: vec![], session_corrections: vec![], diarize: dual,
            idle_config: None, meeting_transcription_language: souffle_lib::settings::MeetingTranscriptionLanguage::Auto,
        }, Box::new(move |segment| {
            let published = Instant::now();
            let elapsed = callback_started.lock().unwrap().elapsed().as_secs_f64();
            let live = callback_live.clone();
            let _ = callback_weak.upgrade_in_event_loop(move |window| {
                eprintln!("UI segment speaker={:?} final={} start={:.3}s end={:.3}s actor_at={elapsed:.3}s slint_queue={:.3}s text={:?}", segment.speaker, segment.is_final, segment.start_time, segment.end_time, published.elapsed().as_secs_f64(), segment.text);
                live_view::apply_live_segment_for_generation(&window, &live, generation, &segment);
                let blocks = window.get_live_transcript_blocks();
                eprintln!("UI blocks={} timeline={:?}", blocks.row_count(), blocks.iter().map(|block|
                    format!("{:?}@{}", block.speaker, block.timestamp)).collect::<Vec<_>>());
            });
        })).unwrap();
        *started.lock().unwrap() = Instant::now();
        let start = Instant::now();
        for (i, chunk) in pcm.chunks(hop).enumerate() {
            let end_sample = i * hop + chunk.len();
            std::thread::sleep(
                (start
                    + Duration::from_secs_f64(
                        end_sample as f64 / f64::from(audio_input.sample_rate_hz),
                    ))
                .saturating_duration_since(Instant::now()),
            );
            let message = match &system_pcm {
                Some(system) => AudioMessage::DiarizedPair {
                    queue_permit: None,
                    session_id: 273,
                    me: chunk.to_vec(),
                    them: system[i * hop..end_sample].to_vec(),
                    captured_at: Instant::now(),
                },
                None => AudioMessage::Chunk(AudioChunk {
                    queue_permit: None,
                    session_id: 273,
                    samples: chunk.to_vec(),
                    captured_at: Instant::now(),
                    speaker: None,
                }),
            };
            tx.send(message).unwrap();
        }
        tx.send(AudioMessage::EndOfStream { session_id: 273 })
            .unwrap();
        let summary = actor.stop_session(Duration::from_secs(60)).unwrap();
        eprintln!("Stopped: {summary:?}");
        actor.shutdown().unwrap();
        std::thread::sleep(Duration::from_secs(hold_seconds));
        slint::quit_event_loop().unwrap();
    });
    window.run().unwrap();
    worker.join().unwrap();
}
