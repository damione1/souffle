//! Measures how much silence Kyutai needs before it ends a sentence.
//!
//! Local only, and ignored by default: it loads the real 2.2 GB STT model, so
//! it never runs in CI. Run it with
//!
//!   cargo test -p souffle --test punctuation_threshold -- --ignored --nocapture
//!
//! This is a characterization test for SOU-030. It reports the measured break
//! point and asserts only the two anchors that held across both sentences.
//! The transition between them is deliberately not asserted: the break is not
//! a pure function of pause length, it also depends on the words, and the fine
//! sweep that produced these fixtures was not monotonic near the boundary.
//!
//! It asserts on punctuation placement, never on which words came back.
//! Synthetic speech is unrepresentatively easy for an ASR, so a word-accuracy
//! assertion here would measure nothing real.

use std::path::{Path, PathBuf};

use souffle_lib::audio::Resampler;
use souffle_lib::engine::kyutai::KyutaiEngine;
use souffle_lib::engine::{
    CANDLE_BACKEND_ID, KYUTAI_ENGINE_ID, KYUTAI_MODEL_ID, TranscriptionEngine,
    resolve_transcription_profile,
};

/// Pause lengths rendered by `tts-fixtures/specs/sou-030-punctuation.toml`.
const GAPS_MS: [u32; 6] = [100, 200, 300, 400, 600, 1000];
const LADDERS: [&str; 2] = ["hesitation-a", "hesitation-b"];

/// The app pads every session with this much trailing silence before flushing.
/// Mirrored here so the measurement matches what dictation actually does.
fn silence_suffix_samples(engine_rate_hz: u32) -> usize {
    let seconds = souffle_lib::constants::SILENCE_SUFFIX_SAMPLES as f64
        / souffle_lib::constants::SAMPLE_RATE_F64;
    (seconds * engine_rate_hz as f64).round() as usize
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/audio/sou-030")
}

fn load_wav_mono(path: &Path) -> (Vec<f32>, u32) {
    let mut reader =
        hound::WavReader::open(path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
    let spec = reader.spec();
    assert_eq!(spec.channels, 1, "{} must be mono", path.display());
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

/// Feed the clip through the engine the way the headless CLI does: native
/// chunk size, trailing silence, then flush.
fn transcribe(engine: &mut KyutaiEngine, pcm: &[f32]) -> String {
    let requirements = engine.audio_requirements();
    let chunk_size = requirements.chunk_size_samples.max(1) as usize;

    let mut padded = pcm.to_vec();
    padded.resize(
        padded.len() + silence_suffix_samples(requirements.sample_rate_hz),
        0.0,
    );

    let mut segments = Vec::new();
    for chunk in padded.chunks(chunk_size) {
        let frame = if chunk.len() < chunk_size {
            let mut buf = chunk.to_vec();
            buf.resize(chunk_size, 0.0);
            buf
        } else {
            chunk.to_vec()
        };
        segments.extend(engine.transcribe(&frame, None).expect("transcribe"));
    }
    segments.extend(engine.flush().expect("flush"));

    // Finals only. The engine also emits each word once as a preview
    // (`is_final: false`) so the UI can show it before it is confirmed;
    // counting those would duplicate every word.
    let parts: Vec<String> = segments
        .iter()
        .filter(|s| s.is_final)
        .map(|s| engine.normalize_text(&s.text))
        .filter(|s| !s.trim().is_empty())
        .collect();
    parts
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// True when the model ended a sentence somewhere other than the very end,
/// which for these two-clause clips means it broke at the pause.
fn has_internal_sentence_break(text: &str) -> bool {
    text.trim_end()
        .trim_end_matches(['.', '!', '?'])
        .contains(['.', '!', '?'])
}

#[test]
#[ignore = "loads the real 2.2 GB Kyutai model; run locally with --ignored"]
fn kyutai_ends_a_sentence_on_a_short_hesitation_pause() {
    let profile = resolve_transcription_profile(
        Some(KYUTAI_ENGINE_ID),
        Some(KYUTAI_MODEL_ID),
        Some(CANDLE_BACKEND_ID),
    )
    .expect("resolve profile");
    assert!(
        souffle_lib::models::model_exists(&profile),
        "Kyutai model missing. Download it in the app, or see the \
         engine-audio-fixtures skill."
    );
    let model_dir = souffle_lib::models::model_dir(&profile);

    let mut engine = KyutaiEngine::new();
    engine.load_model(&model_dir).expect("load model");
    let engine_rate = engine.audio_requirements().sample_rate_hz;

    // Transcribe every rung once, then assert against the table.
    struct Measured {
        ladder: &'static str,
        gap_ms: u32,
        broke: bool,
        text: String,
    }
    let mut measured: Vec<Measured> = Vec::new();

    for ladder in LADDERS {
        println!("\n=== {ladder} ===");
        for gap_ms in GAPS_MS {
            let path = fixtures_dir().join(format!("{ladder}-gap{gap_ms:04}ms.wav"));
            let (samples, rate) = load_wav_mono(&path);
            let pcm = resample(samples, rate, engine_rate);

            engine.reset_state().expect("reset between clips");
            let text = transcribe(&mut engine, &pcm);
            let broke = has_internal_sentence_break(&text);
            println!(
                "  {gap_ms:>5} ms  {}  {text}",
                if broke { "BREAK   " } else { "no break" }
            );
            measured.push(Measured {
                ladder,
                gap_ms,
                broke,
                text,
            });
        }
    }

    let first_break = |ladder: &str| -> Option<u32> {
        measured
            .iter()
            .filter(|m| m.ladder == ladder && m.broke)
            .map(|m| m.gap_ms)
            .min()
    };

    println!("\n--- measured first break ---");
    for ladder in LADDERS {
        match first_break(ladder) {
            Some(ms) => println!("  {ladder}: {ms} ms"),
            None => println!(
                "  {ladder}: no break up to {} ms",
                GAPS_MS[GAPS_MS.len() - 1]
            ),
        }
    }

    let at = |ladder: &str, gap_ms: u32| -> &Measured {
        measured
            .iter()
            .find(|m| m.ladder == ladder && m.gap_ms == gap_ms)
            .unwrap_or_else(|| panic!("no measurement for {ladder} at {gap_ms} ms"))
    };

    // Anchor 1: an ordinary short hesitation is not a sentence end. Held for
    // both sentences at 100 ms and 200 ms when this baseline was recorded.
    for ladder in LADDERS {
        for gap_ms in [100, 200] {
            let m = at(ladder, gap_ms);
            assert!(
                !m.broke,
                "{ladder} at {gap_ms} ms should not end the sentence, got: {}",
                m.text
            );
        }
    }

    // Anchor 2: a full second of silence always ends it. This is the side of
    // the boundary that should stay stable if the threshold is ever tuned.
    for ladder in LADDERS {
        let m = at(ladder, 1000);
        assert!(
            m.broke,
            "{ladder} at 1000 ms should end the sentence, got: {}",
            m.text
        );
    }

    // The finding SOU-030 is about: both sentences break well inside the range
    // of an ordinary mid-thought pause, not at a deliberate full stop.
    // Baseline measured 2026-09-06: hesitation-a 300 ms, hesitation-b 400 ms.
    for ladder in LADDERS {
        let gap = first_break(ladder)
            .unwrap_or_else(|| panic!("{ladder} never broke, the ladder needs longer rungs"));
        assert!(
            gap <= 600,
            "{ladder} first broke at {gap} ms, later than the 600 ms baseline \
             recorded for SOU-030. The threshold appears to have moved."
        );
    }
}

#[test]
fn internal_sentence_break_detection() {
    assert!(!has_internal_sentence_break(
        "Je pense que ce serait une bonne idée, il faudrait en reparler."
    ));
    assert!(has_internal_sentence_break(
        "Je pense que ce serait une bonne idée. Il faudrait en reparler."
    ));
    assert!(!has_internal_sentence_break("Une seule phrase"));
    assert!(!has_internal_sentence_break(""));
    assert!(has_internal_sentence_break("Vraiment ? Oui bien sûr."));
}
