//! Local, weight-backed SOU-273 replay through the production actor/VAD/text filters.
//! Run from app/ so the bundled ORT and Silero resources resolve. Set
//! SOU273_ENGINE=whisper|parakeet, SOU273_SECONDS=600, SOU273_PACED=1 for
//! real-time queue measurements, and SOU273_OUTPUT to save comparable finals.
//! SOU273_DOWNLOAD=1 explicitly permits downloading the catalogue weights.

use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use souffle_lib::audio::{AudioChunk, AudioMessage, Resampler};
use souffle_lib::engine::{
    self, AudioInputRequirements, EngineError, TranscriptionEngine, TranscriptionSegment,
};
use souffle_lib::filter::{PipelineConfig, resolve_vad_model_path};
use souffle_lib::pipeline::{EngineActorHandle, SessionConfig};

#[derive(Default)]
struct Metrics {
    calls: usize,
    decode_s: f64,
    flush_s: f64,
    max_frame_lag_s: f64,
    first_call_s: Option<f64>,
    first_raw_s: Option<f64>,
    first_output_call_s: Option<f64>,
    first_output_inference_s: Option<f64>,
    raw_segments: usize,
    samples_fed: usize,
    first_filtered_s: Option<f64>,
    revisions: Vec<f64>,
    segments: Vec<TranscriptionSegment>,
}

struct MeasuredEngine {
    inner: Box<dyn TranscriptionEngine>,
    metrics: Arc<Mutex<Metrics>>,
    started: Arc<Mutex<Instant>>,
    previews: bool,
}

impl TranscriptionEngine for MeasuredEngine {
    fn set_preview_enabled(&mut self, enabled: bool) {
        self.inner.set_preview_enabled(enabled && self.previews);
    }
    fn minimum_vad_hold_seconds(&self) -> f64 {
        self.inner.minimum_vad_hold_seconds()
    }
    fn load_model(&mut self, p: &Path) -> Result<(), EngineError> {
        self.inner.load_model(p)
    }
    fn unload_model(&mut self) -> Result<(), EngineError> {
        self.inner.unload_model()
    }
    fn reset_state(&mut self) -> Result<(), EngineError> {
        self.inner.reset_state()
    }
    fn reset_state_preserving_timeline(&mut self) -> Result<(), EngineError> {
        self.inner.reset_state_preserving_timeline()
    }
    fn audio_requirements(&self) -> AudioInputRequirements {
        self.inner.audio_requirements()
    }
    fn normalize_text(&self, t: &str) -> String {
        self.inner.normalize_text(t)
    }
    fn transcribe(
        &mut self,
        audio: &[f32],
        language: Option<&str>,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let start = Instant::now();
        let since_start = self.started.lock().unwrap().elapsed().as_secs_f64();
        let result = self.inner.transcribe(audio, language);
        let mut m = self.metrics.lock().unwrap();
        let sample_clock_s = (m.samples_fed + audio.len()) as f64 / 16_000.0;
        m.max_frame_lag_s = m
            .max_frame_lag_s
            .max((since_start - sample_clock_s).max(0.0));
        m.calls += 1;
        m.samples_fed += audio.len();
        m.first_call_s.get_or_insert(since_start);
        m.decode_s += start.elapsed().as_secs_f64();
        if let Ok(segments) = &result {
            m.raw_segments += segments.len();
            if !segments.is_empty() {
                m.first_output_call_s.get_or_insert(since_start);
                m.first_output_inference_s
                    .get_or_insert(start.elapsed().as_secs_f64());
                m.first_raw_s
                    .get_or_insert(self.started.lock().unwrap().elapsed().as_secs_f64());
            }
        }
        result
    }
    fn flush(&mut self) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let started = Instant::now();
        let result = self.inner.flush();
        self.metrics.lock().unwrap().flush_s += started.elapsed().as_secs_f64();
        result
    }
}

fn fixture(file: &str) -> Vec<f32> {
    let mut reader = hound::WavReader::open(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/audio")
            .join(file),
    )
    .unwrap();
    let rate = reader.spec().sample_rate;
    let pcm: Vec<_> = reader
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / f32::from(i16::MAX))
        .collect();
    if rate == 16_000 {
        return pcm;
    }
    let mut resampler = Resampler::new(rate, 1, 16_000, 1.0);
    let mut out = resampler.process(&pcm);
    out.extend(resampler.flush());
    out
}

fn assert_same_finals(before: &[TranscriptionSegment], after: &[TranscriptionSegment]) {
    assert_eq!(before.len(), after.len(), "final count changed");
    for (a, b) in before.iter().zip(after) {
        assert_eq!(a.text, b.text);
        assert_eq!(a.speaker, b.speaker);
        assert_eq!(a.language, b.language);
        assert!(
            (a.start_time - b.start_time).abs() < 0.01,
            "start timestamp changed: {a:?} / {b:?}"
        );
        assert!(
            (a.end_time - b.end_time).abs() < 0.01,
            "end timestamp changed: {a:?} / {b:?}"
        );
    }
}

fn percentile(mut samples: Vec<f64>, rank: f64) -> Option<f64> {
    samples.sort_by(f64::total_cmp);
    (!samples.is_empty()).then(|| samples[((samples.len() - 1) as f64 * rank).ceil() as usize])
}

fn peak_resident_mib() -> f64 {
    // SAFETY: getrusage writes a valid rusage structure into an aligned,
    // initialized allocation; RUSAGE_SELF has no additional preconditions.
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        assert_eq!(libc::getrusage(libc::RUSAGE_SELF, &mut usage), 0);
        // macOS reports ru_maxrss in bytes.
        usage.ru_maxrss as f64 / (1024.0 * 1024.0)
    }
}

#[test]
#[ignore = "requires catalogue weights; real decoder silence/tail/final equivalence"]
fn weight_backed_batch_final_equivalence_and_silence() {
    whisper_rs::install_logging_hooks();
    let engine_id = std::env::var("SOU273_ENGINE").expect("set SOU273_ENGINE");
    let profile = engine::resolve_transcription_profile(Some(&engine_id), None, None).unwrap();
    let mut decoder = engine::create_engine(&profile).unwrap();
    decoder
        .load_model(&souffle_lib::models::model_dir(&profile))
        .unwrap();
    let mut pcm = fixture("sou-030/hesitation-a-gap0400ms.wav");
    pcm.extend(fixture("sou-184/ecorp-q4-intro.wav"));
    pcm.truncate(30 * 16_000 + 3200); // short final remainder (< both inference floors)
    let mut baseline = Vec::new();
    for previews in [false, true] {
        decoder.reset_state().unwrap();
        decoder.set_preview_enabled(previews);
        let mut finals = Vec::new();
        let mut preview_count = 0;
        for chunk in pcm.chunks(1600) {
            let segments = decoder.transcribe(chunk, None).unwrap();
            preview_count += segments
                .iter()
                .filter(|s| !s.is_final && !s.text.is_empty())
                .count();
            finals.extend(segments.into_iter().filter(|s| s.is_final));
        }
        finals.extend(decoder.flush().unwrap());
        assert!(
            decoder.flush().unwrap().is_empty(),
            "flush duplicates finals"
        );
        if previews {
            assert!(preview_count > 0);
            assert_same_finals(&baseline, &finals);
            println!(
                "{engine_id}: {} identical finals with {preview_count} previews, including short stop tail",
                finals.len()
            );
        } else {
            baseline = finals;
        }
    }
    decoder.reset_state().unwrap();
    for _ in 0..75 {
        assert!(
            decoder
                .transcribe(&[0.0; 1600], None)
                .unwrap()
                .iter()
                .all(|s| s.text.is_empty())
        );
    }
    assert!(
        decoder.flush().unwrap().is_empty(),
        "silence flush hallucinated"
    );
    println!("{engine_id}: 7.5 s digital silence produces no words");
}

#[test]
#[ignore = "requires Whisper/Parakeet weights; optional ten-minute paced replay"]
fn production_batch_preview_replay() {
    whisper_rs::install_logging_hooks();
    let engine_id = std::env::var("SOU273_ENGINE").expect("set SOU273_ENGINE");
    let profile = engine::resolve_transcription_profile(Some(&engine_id), None, None).unwrap();
    if !souffle_lib::models::model_exists(&profile) {
        assert_eq!(
            std::env::var("SOU273_DOWNLOAD").as_deref(),
            Ok("1"),
            "weights missing: use SOU273_DOWNLOAD=1"
        );
        souffle_lib::models::download_model(&profile, |_| {}).unwrap();
    }
    let seconds: usize = std::env::var("SOU273_SECONDS")
        .unwrap_or_else(|_| "30".into())
        .parse()
        .unwrap();
    let paced = std::env::var("SOU273_PACED").as_deref() == Ok("1");
    let previews = std::env::var("SOU273_PREVIEWS").as_deref() != Ok("0");
    let mut source = fixture("sou-030/hesitation-a-gap0400ms.wav");
    source.extend(fixture("sou-184/ecorp-q4-intro.wav"));
    let pcm: Vec<f32> = source
        .iter()
        .copied()
        .cycle()
        .take(seconds * 16_000)
        .collect();
    if let Ok(path) = std::env::var("SOU273_PCM") {
        std::fs::write(path, serde_json::to_vec(&pcm).unwrap()).unwrap();
    }
    let started = Arc::new(Mutex::new(Instant::now()));
    let metrics = Arc::new(Mutex::new(Metrics::default()));
    let factory_metrics = metrics.clone();
    let factory_started = started.clone();
    let (tx, rx) = crossbeam_channel::bounded(200); // bounded audio queue, no preview-job queue
    let dropped = Arc::new(AtomicU64::new(0));
    let actor = EngineActorHandle::spawn(
        rx,
        dropped.clone(),
        Arc::new(Mutex::new(None)),
        Box::new(move |p| {
            Ok(Box::new(MeasuredEngine {
                inner: engine::create_engine(p)?,
                metrics: factory_metrics.clone(),
                started: factory_started.clone(),
                previews,
            }))
        }),
    )
    .unwrap();
    actor
        .load_model(profile.clone(), souffle_lib::models::model_dir(&profile))
        .unwrap();
    let sink = metrics.clone();
    let callback_started = started.clone();
    let vad_path = resolve_vad_model_path().expect("bundled Silero VAD");
    println!(
        "profile={profile:?} vad={vad_path:?} seconds={seconds} paced={paced} previews={previews}"
    );
    actor
        .start_session(
            273,
            SessionConfig {
                pipeline_config: PipelineConfig {
                    vad_enabled: true,
                    vad_model_path: Some(vad_path),
                    filler_removal_enabled: true,
                    stutter_collapse_enabled: true,
                    dictionary_correction_enabled: false,
                },
                dictionary_entries: vec![],
                session_terms: vec![],
                session_corrections: vec![],
                diarize: false,
                idle_config: None,
                meeting_transcription_language:
                    souffle_lib::settings::MeetingTranscriptionLanguage::Auto,
            },
            Box::new(move |s| {
                let elapsed = callback_started.lock().unwrap().elapsed().as_secs_f64();
                let mut m = sink.lock().unwrap();
                if !s.text.is_empty() {
                    m.first_filtered_s.get_or_insert(elapsed);
                }
                if !s.is_final && !s.text.is_empty() {
                    m.revisions.push(elapsed);
                }
                m.segments.push(s);
            }),
        )
        .unwrap();
    *started.lock().unwrap() = Instant::now();
    let start = Instant::now();
    let mut max_queue = 0;
    for (i, chunk) in pcm.chunks(1600).enumerate() {
        if paced {
            let due = start + Duration::from_millis((i as u64 + 1) * 100);
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
        }
        let message = AudioMessage::Chunk(AudioChunk {
            queue_permit: None,
            session_id: 273,
            samples: chunk.to_vec(),
            captured_at: Instant::now(),
            speaker: None,
        });
        if paced {
            if tx.try_send(message).is_err() {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
        } else {
            tx.send(message).unwrap();
        }
        max_queue = max_queue.max(tx.len());
    }
    tx.send(AudioMessage::EndOfStream { session_id: 273 })
        .unwrap();
    let summary = actor.stop_session(Duration::from_secs(900)).unwrap();
    let elapsed = start.elapsed().as_secs_f64();
    actor.shutdown().unwrap();
    let m = metrics.lock().unwrap();
    let finals: Vec<_> = m.segments.iter().filter(|s| s.is_final).collect();
    println!(
        "wall={elapsed:.3}s transcribe_cost={:.3}s calls={} first_engine_call={:?} first_raw={:?} first_filtered={:?} previews={} finals={} raw_segments={} max_queue={} summary={summary:?}",
        m.decode_s,
        m.calls,
        m.first_call_s,
        m.first_raw_s,
        m.first_filtered_s,
        m.revisions.len(),
        finals.len(),
        m.raw_segments,
        max_queue
    );
    assert!(!finals.is_empty(), "fixture must produce final speech");
    let intervals: Vec<_> = m
        .revisions
        .windows(2)
        .map(|times| times[1] - times[0])
        .collect();
    println!(
        "revision_interval_p50={:?} p95={:?} max_frame_lag={:.3}s flush_cost={:.3}s peak_resident={:.1}MiB",
        percentile(intervals.clone(), 0.5),
        percentile(intervals, 0.95),
        m.max_frame_lag_s,
        m.flush_s,
        peak_resident_mib()
    );
    println!(
        "captured_samples={} engine_samples={} vad_evicted_samples={}",
        pcm.len(),
        m.samples_fed,
        pcm.len().saturating_sub(m.samples_fed)
    );
    println!(
        "first_output_audio_wait={:?} first_output_engine_cost={:?}",
        m.first_output_call_s, m.first_output_inference_s
    );
    assert_eq!(summary.dropped_chunks, 0, "no capture loss accepted");
    if let Ok(path) = std::env::var("SOU273_COMPARE") {
        let baseline: Vec<TranscriptionSegment> =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let actual: Vec<_> = finals.iter().map(|s| (*s).clone()).collect();
        assert_same_finals(&baseline, &actual);
        println!(
            "Final text/language/speaker/timestamps match baseline at existing 10 ms precision"
        );
    }
    if let Ok(path) = std::env::var("SOU273_OUTPUT") {
        std::fs::write(path, serde_json::to_vec_pretty(&finals).unwrap()).unwrap();
    }
}
