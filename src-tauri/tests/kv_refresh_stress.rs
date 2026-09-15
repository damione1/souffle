//! KV refresh policy stress test — multi-speaker E Corp meeting fixtures.
//!
//! Validates that the SoftOnly KV refresh policy (SOU-181) does not produce
//! long mute windows or crashes on realistic multi-speaker FR/EN audio.
//!
//! Local only, ignored by default (loads the real ~2.2 GB Kyutai STT model):
//!
//!   cargo test -p souffle --test kv_refresh_stress -- --ignored --nocapture
//!
//! Generate fixtures first if missing:
//!   cargo run -p souffle-tts-fixtures -- tts-fixtures/specs/sou-184-ecorp-meeting.toml

use std::path::{Path, PathBuf};
use std::time::Instant;

use souffle_lib::audio::Resampler;
use souffle_lib::engine::kyutai::KyutaiEngine;
use souffle_lib::engine::{
    CANDLE_BACKEND_ID, KYUTAI_ENGINE_ID, KYUTAI_MODEL_ID, TranscriptionEngine,
    resolve_transcription_profile,
};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/audio/sou-184")
}

fn fixture_path(name: &str) -> PathBuf {
    fixtures_dir().join(format!("{name}.wav"))
}

fn require_fixture(name: &str) {
    let path = fixture_path(name);
    assert!(
        path.is_file(),
        "missing fixture: {}\nGenerate with: cargo run -p souffle-tts-fixtures -- \
         tts-fixtures/specs/sou-184-ecorp-meeting.toml",
        path.display()
    );
}

fn load_wav_mono_f32(path: &Path) -> (Vec<f32>, u32) {
    let mut reader =
        hound::WavReader::open(path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
    let spec = reader.spec();
    assert_eq!(spec.channels, 1, "{} must be mono", path.display());
    assert_eq!(
        spec.bits_per_sample,
        16,
        "{} must be 16-bit PCM",
        path.display()
    );
    assert_eq!(
        spec.sample_format,
        hound::SampleFormat::Int,
        "{} must be integer PCM",
        path.display()
    );
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.expect("read sample") as f32 / i16::MAX as f32)
        .collect();
    (samples, spec.sample_rate)
}

fn resample(samples: Vec<f32>, from: u32, to: u32) -> Vec<f32> {
    if from == to || samples.is_empty() {
        return samples;
    }
    let mut resampler = Resampler::new(from, 1, to, 1.0);
    let mut out = resampler.process(&samples);
    out.extend(resampler.flush());
    out
}

/// Metrics collected during a stress run.
#[derive(Debug, Default)]
struct StressMetrics {
    final_segments: usize,
    max_gap_s: f64,
    audio_duration_s: f64,
    wall_s: f64,
}

impl StressMetrics {
    fn print(&self, label: &str) {
        println!("\n=== {label} ===");
        println!("  audio duration : {:.1} s", self.audio_duration_s);
        println!("  wall time      : {:.1} s", self.wall_s);
        println!("  final segments : {}", self.final_segments);
        println!("  max gap        : {:.2} s", self.max_gap_s);
    }
}

fn run_stress(engine: &mut KyutaiEngine, pcm: &[f32], audio_duration_s: f64) -> StressMetrics {
    let requirements = engine.audio_requirements();
    let chunk_size = requirements.chunk_size_samples.max(1) as usize;

    // Silence suffix mirrors the app's own padding.
    let silence_samples = {
        let seconds = souffle_lib::constants::SILENCE_SUFFIX_SAMPLES as f64
            / souffle_lib::constants::SAMPLE_RATE_F64;
        (seconds * requirements.sample_rate_hz as f64).round() as usize
    };
    let mut padded = pcm.to_vec();
    padded.resize(padded.len() + silence_samples, 0.0);

    let t0 = Instant::now();
    let mut all_segments = Vec::new();
    for chunk in padded.chunks(chunk_size) {
        let frame = if chunk.len() < chunk_size {
            let mut buf = chunk.to_vec();
            buf.resize(chunk_size, 0.0);
            buf
        } else {
            chunk.to_vec()
        };
        let segs = engine.transcribe(&frame, None).expect("transcribe");
        all_segments.extend(segs);
    }
    all_segments.extend(engine.flush().expect("flush"));
    let wall_s = t0.elapsed().as_secs_f64();

    // Compute metrics from final segments only.
    let mut finals: Vec<_> = all_segments.into_iter().filter(|s| s.is_final).collect();
    finals.sort_by(|a, b| a.start_time.partial_cmp(&b.start_time).unwrap());

    let mut max_gap_s: f64 = 0.0;
    let mut prev_end: Option<f64> = None;

    for seg in &finals {
        if let Some(prev) = prev_end {
            let gap = seg.start_time - prev;
            if gap > max_gap_s {
                max_gap_s = gap;
            }
        }
        prev_end = Some(seg.end_time);
    }

    StressMetrics {
        final_segments: finals.len(),
        max_gap_s,
        audio_duration_s,
        wall_s,
    }
}

fn load_engine() -> KyutaiEngine {
    let profile = resolve_transcription_profile(
        Some(KYUTAI_ENGINE_ID),
        Some(KYUTAI_MODEL_ID),
        Some(CANDLE_BACKEND_ID),
    )
    .expect("resolve profile");
    assert!(
        souffle_lib::models::model_exists(&profile),
        "Kyutai model missing. Download it in the app."
    );
    let model_dir = souffle_lib::models::model_dir(&profile);
    let mut engine = KyutaiEngine::new();
    engine.load_model(&model_dir).expect("load model");
    engine
}

#[test]
#[ignore = "loads real Kyutai model and fixture; run locally with --ignored"]
fn ecorp_intro_no_long_gap() {
    let name = "ecorp-q4-intro";
    require_fixture(name);
    let mut engine = load_engine();
    let engine_rate = engine.audio_requirements().sample_rate_hz;

    let path = fixture_path(name);
    let (samples, rate) = load_wav_mono_f32(&path);
    let audio_duration_s = samples.len() as f64 / rate as f64;
    let pcm = resample(samples, rate, engine_rate);

    let metrics = run_stress(&mut engine, &pcm, audio_duration_s);
    metrics.print("SOU-184 / ecorp-q4-intro");

    assert!(
        metrics.max_gap_s < 4.0,
        "max gap {:.2} s >= 4 s threshold",
        metrics.max_gap_s
    );
}

#[test]
#[ignore = "loads real Kyutai model; run locally with --ignored"]
fn ecorp_incident_fren_no_long_gap() {
    let name = "ecorp-q4-incident";
    require_fixture(name);
    let mut engine = load_engine();
    let engine_rate = engine.audio_requirements().sample_rate_hz;

    let path = fixture_path(name);
    let (samples, rate) = load_wav_mono_f32(&path);
    let audio_duration_s = samples.len() as f64 / rate as f64;
    let pcm = resample(samples, rate, engine_rate);

    let metrics = run_stress(&mut engine, &pcm, audio_duration_s);
    metrics.print("SOU-184 / ecorp-q4-incident");

    assert!(
        metrics.max_gap_s < 5.0,
        "max gap {:.2} s >= 5 s threshold",
        metrics.max_gap_s
    );
}

#[test]
#[ignore = "loads real Kyutai model; run locally with --ignored"]
fn ecorp_three_meetings_back_to_back() {
    let mut engine = load_engine();
    let engine_rate = engine.audio_requirements().sample_rate_hz;

    let fixtures = ["ecorp-q4-intro", "ecorp-q4-incident", "ecorp-q4-close"];

    for (i, name) in fixtures.iter().enumerate() {
        require_fixture(name);
        let path = fixture_path(name);
        let (samples, rate) = load_wav_mono_f32(&path);
        let audio_duration_s = samples.len() as f64 / rate as f64;
        let pcm = resample(samples, rate, engine_rate);

        if i > 0 {
            engine.reset_state().expect("reset state");
        }

        let metrics = run_stress(&mut engine, &pcm, audio_duration_s);
        metrics.print(&format!("SOU-184 / back-to-back: {name}"));

        assert!(
            metrics.max_gap_s < 5.0,
            "{name}: max gap {:.2} s >= 5 s",
            metrics.max_gap_s
        );
    }
}
