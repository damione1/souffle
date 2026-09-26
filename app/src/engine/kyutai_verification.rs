//! Opt-in, deterministic verification through production transcribe/flush.
//! This harness has no policy override and is absent from shipped builds.
use super::*;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    io::{BufWriter, Write},
    path::PathBuf,
};

thread_local! {
    static EVENTS: RefCell<Option<BufWriter<File>>> = const { RefCell::new(None) };
}
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum TraceKind {
    Source,
    Eos,
    Tokens,
    Word,
    EndWord,
    State,
}

fn event(kind: TraceKind, mut value: Value) {
    value["event"] = serde_json::to_value(kind).unwrap();
    EVENTS.with(|slot| {
        if let Some(file) = slot.borrow_mut().as_mut() {
            writeln!(file, "{value}").unwrap();
        }
    });
}
pub(super) fn tokens(items: &[moshi::asr::ItemState]) {
    // Moshi's callback runs before forward: selected text is delayed one step.
    event(
        TraceKind::Tokens,
        json!({ "ids":items.iter().map(moshi::asr::ItemState::text_token).collect::<Vec<_>>()}),
    );
}
pub(super) fn mapped(kind: TraceKind, lane: usize, raw: f64, delivered: f64, text: Option<&str>) {
    event(
        kind,
        json!({"lane":lane,"raw":raw,"delivered":delivered,"clamp":delivered-raw,"text":text}),
    );
}
fn normalized(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect()
}
fn planned(intervals: &[(f64, f64)], from: f64, to: f64) -> f64 {
    intervals
        .iter()
        .map(|&(s, e)| (to.min(e) - from.max(s)).max(0.0))
        .sum()
}
fn lcs(reference: &[String], observed: &[String]) -> usize {
    let mut row = vec![0; observed.len() + 1];
    for word in reference {
        let mut previous = 0;
        for (i, value) in observed.iter().enumerate() {
            let saved = row[i + 1];
            row[i + 1] = if word == value {
                previous + 1
            } else {
                row[i + 1].max(row[i])
            };
            previous = saved;
        }
    }
    row[observed.len()]
}

#[test]
#[ignore = "requires Nightly model, ECorp fixture and original E1 baseline artifacts"]
fn language_mismatch_fixture_preserves_text_and_finals() {
    let fixture = PathBuf::from(std::env::var_os("SOUFFLE_VERIFY_FIXTURE").unwrap());
    let model_path = PathBuf::from(std::env::var_os("SOUFFLE_VERIFY_MODEL").unwrap());
    let baseline = PathBuf::from(std::env::var_os("SOUFFLE_VERIFY_BASELINE").unwrap());
    let dir = PathBuf::from(std::env::var_os("SOUFFLE_VERIFY_DIR").unwrap());
    std::fs::create_dir_all(&dir).unwrap();
    let mut reader = hound::WavReader::open(fixture).unwrap();
    let pcm: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / i16::MAX as f32)
        .collect();
    let mut engine = KyutaiEngine::new();
    engine.load_model(&model_path).unwrap();
    EVENTS.with(|slot| {
        *slot.borrow_mut() = Some(BufWriter::new(
            File::create(dir.join("events.jsonl")).unwrap(),
        ))
    });
    let mut segments = Vec::new();
    let mut accepted = 0;
    let started = std::time::Instant::now();
    for chunk in pcm.chunks(MIMI_FRAME_SIZE) {
        accepted += chunk.len();
        event(
            TraceKind::Source,
            json!({"cursor":accepted as f64 / SAMPLE_RATE as f64}),
        );
        segments.extend(engine.transcribe(chunk, None).unwrap());
    }
    segments.extend(engine.flush().unwrap());
    EVENTS.with(|slot| slot.borrow_mut().take().unwrap().flush().unwrap());
    let expected: Vec<(String, bool)> = std::fs::read_to_string(baseline)
        .unwrap()
        .lines()
        .map(|line| {
            let record: Value = serde_json::from_str(line).unwrap();
            (
                record["segment"]["text"].as_str().unwrap().to_string(),
                record["segment"]["is_final"].as_bool().unwrap(),
            )
        })
        .collect();
    let observed: Vec<_> = segments
        .iter()
        .map(|segment| (segment.text.clone(), segment.is_final))
        .collect();
    assert_eq!(observed, expected);
    let mut clamps = 0;
    let mut future = 0;
    let mut cursor = 0.0;
    for line in std::fs::read_to_string(dir.join("events.jsonl"))
        .unwrap()
        .lines()
    {
        let record: Value = serde_json::from_str(line).unwrap();
        let kind: TraceKind = serde_json::from_value(record["event"].clone()).unwrap();
        match kind {
            TraceKind::Source => cursor = record["cursor"].as_f64().unwrap(),
            TraceKind::Word | TraceKind::EndWord => {
                clamps += usize::from(record["clamp"].as_f64().unwrap() > 1e-8);
                future += usize::from(record["delivered"].as_f64().unwrap() > cursor + 1e-8);
            }
            TraceKind::Eos | TraceKind::State | TraceKind::Tokens => {}
        }
    }
    assert_eq!(clamps, 0);
    assert_eq!(future, 0);
    let summary = json!({"prior":"Auto","accepted_samples":accepted,"segments":segments.len(),"finals":segments.iter().filter(|segment| segment.is_final).count(),"text_and_final_flags_identical_to_E1_baseline":true,"clamps":clamps,"future_times":future,"wall_seconds":started.elapsed().as_secs_f64()});
    std::fs::write(
        dir.join("summary.json"),
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .unwrap();
    println!("{summary}");
    engine.unload_model().unwrap();
}

#[test]
#[ignore = "requires local Nightly weights and long fixture; one Metal job at a time"]
fn soft_only_full_fixture() {
    crate::debug::set_transcription_debug(false);
    let fixture = PathBuf::from(std::env::var_os("SOUFFLE_VERIFY_FIXTURE").expect("fixture WAV"));
    let model_path =
        PathBuf::from(std::env::var_os("SOUFFLE_VERIFY_MODEL").expect("Nightly model directory"));
    let dir = PathBuf::from(std::env::var_os("SOUFFLE_VERIFY_DIR").expect("output directory"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut reader = hound::WavReader::open(&fixture).unwrap();
    assert_eq!(reader.spec().sample_rate, SAMPLE_RATE);
    assert_eq!(reader.spec().channels, 1);
    let pcm: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / i16::MAX as f32)
        .collect();
    let duration = pcm.len() as f64 / SAMPLE_RATE as f64;
    let intervals: Vec<(f64, f64)> = std::fs::read_to_string(fixture.with_extension("timings.tsv"))
        .unwrap()
        .lines()
        .skip(1)
        .map(|line| {
            let columns: Vec<_> = line.split('\t').collect();
            (
                columns[1].parse::<usize>().unwrap() as f64 / SAMPLE_RATE as f64,
                columns[2].parse::<usize>().unwrap() as f64 / SAMPLE_RATE as f64,
            )
        })
        .collect();
    assert_eq!(intervals.len(), 18);
    assert!(duration > 460.0);
    let mut engine = KyutaiEngine::new();
    engine.set_meeting_language_prior(MeetingTranscriptionLanguage::Fr);
    engine.load_model(&model_path).unwrap();
    assert!(matches!(
        engine.model.as_ref().unwrap().device,
        Device::Metal(_)
    ));
    EVENTS.with(|slot| {
        *slot.borrow_mut() = Some(BufWriter::new(
            File::create(dir.join("events.jsonl")).unwrap(),
        ))
    });
    let started = std::time::Instant::now();
    let mut accepted = 0;
    let mut segments_file = BufWriter::new(File::create(dir.join("segments.jsonl")).unwrap());
    let mut transcript = String::new();
    let mut previews = 0;
    let mut finals = 0;
    for chunk in pcm.chunks(MIMI_FRAME_SIZE) {
        accepted += chunk.len();
        let cursor = accepted as f64 / SAMPLE_RATE as f64;
        event(
            TraceKind::Source,
            json!({"cursor":cursor,"samples":chunk.len(),"rms":pcm_rms(chunk)}),
        );
        for segment in engine.transcribe(chunk, None).unwrap() {
            if segment.is_final {
                finals += 1;
                transcript.push_str(&segment.text);
                transcript.push(' ');
            } else {
                previews += 1;
            }
            writeln!(
                segments_file,
                "{}",
                json!({"phase":"source","cursor":cursor,"segment":segment})
            )
            .unwrap();
        }
        let m = engine.model.as_ref().unwrap();
        event(
            TraceKind::State,
            json!({"step":m.state.model_step_idx(),"frames":m.frames_since_refresh,"refreshes":m.refresh_count,"origin":m.epoch_origin_seconds}),
        );
    }
    event(
        TraceKind::Eos,
        json!({"cursor":duration,"accepted":accepted,"dropped_samples":0,"added_source_silence":0}),
    );
    for segment in engine.flush().unwrap() {
        if segment.is_final {
            finals += 1;
            transcript.push_str(&segment.text);
            transcript.push(' ');
        } else {
            previews += 1;
        }
        writeln!(
            segments_file,
            "{}",
            json!({"phase":"flush","cursor":duration,"segment":segment})
        )
        .unwrap();
    }
    segments_file.flush().unwrap();
    EVENTS.with(|slot| {
        slot.borrow_mut().take().unwrap().flush().unwrap();
    });
    let elapsed = started.elapsed().as_secs_f64();
    let mut cursor = 0.0;
    let mut phase = 0;
    let mut counts = [[0usize; 4]; 2]; // Word, EndWord, callback, delimiter token
    let mut bins = vec![[0usize; 3]; (duration / 15.0).ceil() as usize]; // Words, callbacks, delimiters
    let mut last_word = 0.0;
    let mut last_lexical = 0.0;
    let mut previous_word = Vec::new();
    let mut max_gap = 0f64;
    let mut first_famine = None;
    let mut clamps = 0;
    let mut future = 0;
    let mut max_non_delimiter = 0;
    let mut non_delimiter = 0;
    let mut repeated = 0;
    let mut max_repeated = 0;
    let mut previous_token = None;
    let mut active_seconds = 0.0;
    for line in std::fs::read_to_string(dir.join("events.jsonl"))
        .unwrap()
        .lines()
    {
        let e: Value = serde_json::from_str(line).unwrap();
        let bin = ((cursor / 15.0) as usize).min(bins.len() - 1);
        let kind: TraceKind = serde_json::from_value(e["event"].clone()).unwrap();
        match kind {
            TraceKind::Source => {
                cursor = e["cursor"].as_f64().unwrap();
                if e["rms"].as_f64().unwrap() >= ENERGY_PAUSE_RMS as f64 {
                    active_seconds += e["samples"].as_u64().unwrap() as f64 / SAMPLE_RATE as f64;
                }
            }
            TraceKind::Eos => phase = 1,
            TraceKind::Tokens => {
                counts[phase][2] += 1;
                if phase == 0 {
                    bins[bin][1] += 1;
                }
                let token = e["ids"][0].as_u64().unwrap();
                // Moshi's ASR assembler closes words on ids 0 and 3.
                if token == 0 || token == 3 {
                    counts[phase][3] += 1;
                    non_delimiter = 0;
                    repeated = 0;
                    if phase == 0 {
                        bins[bin][2] += 1;
                    }
                } else {
                    non_delimiter += 1;
                    repeated = if previous_token == Some(token) {
                        repeated + 1
                    } else {
                        1
                    };
                }
                max_non_delimiter = max_non_delimiter.max(non_delimiter);
                max_repeated = max_repeated.max(repeated);
                previous_token = Some(token);
            }
            TraceKind::Word | TraceKind::EndWord => {
                let is_word = kind == TraceKind::Word;
                counts[phase][usize::from(!is_word)] += 1;
                clamps += usize::from(e["clamp"].as_f64().unwrap() > 1e-8);
                future += usize::from(e["delivered"].as_f64().unwrap() > cursor + 1e-8);
                if is_word && phase == 0 {
                    bins[bin][0] += 1;
                    last_word = cursor;
                    let word = normalized(e["text"].as_str().unwrap());
                    if !word.is_empty() && word != previous_word {
                        max_gap = max_gap.max(planned(&intervals, last_lexical, cursor));
                        last_lexical = cursor;
                        previous_word = word;
                    }
                }
            }
            TraceKind::State => {}
        }
        if phase == 0 && first_famine.is_none() && planned(&intervals, last_lexical, cursor) >= 15.0
        {
            first_famine = Some(cursor);
        }
    }
    max_gap = max_gap.max(planned(&intervals, last_lexical, duration));
    let reference_text = std::fs::read_to_string(fixture.with_extension("reference.txt")).unwrap();
    let reference = normalized(&reference_text);
    let observed = normalized(&transcript);
    let aligned = lcs(&reference, &observed);
    let sentinels = [
        "azur",
        "cuivre",
        "corail",
        "ivoire",
        "olive",
        "indigo",
        "ambre",
        "jade",
        "nacre",
        "turquoise",
        "grenat",
        "argent",
        "bronze",
        "cobalt",
        "rubis",
        "saphir",
        "émeraude",
        "cristal",
    ];
    let missing_sentinels: Vec<_> = sentinels
        .iter()
        .filter(|word| !observed.iter().any(|value| value == **word))
        .collect();
    let model = engine.model.as_ref().unwrap();
    let summary = json!({"policy":"production SoftOnly, no override","prior":"Fr","temperature":0.0,"backend":"Metal","dtype":format!("{:?}",model.device.bf16_default_to_f32()),"context":model.config.context,"audio_seconds":duration,"planned_speech_seconds":planned(&intervals,0.0,duration),"accepted_samples":accepted,"lost_samples":0,"source_calls":pcm.chunks(MIMI_FRAME_SIZE).len(),"refreshes":model.refresh_count,"step_after_flush":model.state.model_step_idx(),"frames_after_flush":model.frames_since_refresh,"origins":model.epoch_origin_seconds,"previews":previews,"finals":finals,"phase_counts":counts,"phase_columns":["Word","EndWord","callback","delimiter"],"bins_15s":bins,"bin_columns":["Word","callback","delimiter"],"clamps":clamps,"future_times":future,"last_source_word_cursor":last_word,"last_lexical_cursor":last_lexical,"first_15s_famine":first_famine,"max_planned_lexical_gap":max_gap,"active_rms_seconds":active_seconds,"max_non_delimiter_run":max_non_delimiter,"max_repeated_non_delimiter_token":max_repeated,"reference_words":reference.len(),"transcript_words":observed.len(),"lcs_matched_reference_words":aligned,"unmatched_reference_words":reference.len()-aligned,"missing_sentinels":missing_sentinels,"lexical_alignment_limit":"LCS is content screening, not phoneme alignment or WER","token_limit":"callback observes previous selection, final selected token not observable","wall_seconds":elapsed});
    std::fs::write(
        dir.join("summary.json"),
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .unwrap();
    println!("{summary}");
    engine.unload_model().unwrap();
}
