//! Text metrics are ASR observations, not acoustic speaker attribution.
//! Shared gold words are ambiguous and excluded from remote-overlap counts.
use crate::{
    Result,
    protocol::{self, Mode, Protocol, Scene, Track, TrialStatus},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use souffle_lib::engine::{PARAKEET_ENGINE_ID, WHISPER_ENGINE_ID};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

fn words(text: &str) -> Vec<String> {
    // Remove punctuation, retain Unicode letters/numbers and accents. Whitespace
    // is the only token separator; apostrophes/hyphens are removed, not split.
    text.nfc()
        .collect::<String>()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || is_combining_mark(*c) || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}
#[derive(Clone, Copy, Default, Debug, Serialize, PartialEq)]
struct Edits {
    substitutions: usize,
    deletions: usize,
    insertions: usize,
}
impl Edits {
    fn total(self) -> usize {
        self.substitutions + self.deletions + self.insertions
    }
}
fn edits(reference: &[String], hypothesis: &[String]) -> Edits {
    let mut prev: Vec<Edits> = (0..=hypothesis.len())
        .map(|i| Edits {
            insertions: i,
            ..Edits::default()
        })
        .collect();
    for (i, r) in reference.iter().enumerate() {
        let mut row = vec![Edits {
            deletions: i + 1,
            ..Edits::default()
        }];
        for (j, h) in hypothesis.iter().enumerate() {
            if r == h {
                row.push(prev[j]);
                continue;
            }
            let sub = Edits {
                substitutions: prev[j].substitutions + 1,
                ..prev[j]
            };
            let del = Edits {
                deletions: prev[j + 1].deletions + 1,
                ..prev[j + 1]
            };
            let ins = Edits {
                insertions: row[j].insertions + 1,
                ..row[j]
            };
            // Fixed tie order: substitution, deletion, insertion.
            row.push(
                [sub, del, ins]
                    .into_iter()
                    .min_by_key(|e| e.total())
                    .unwrap(),
            );
        }
        prev = row;
    }
    prev[hypothesis.len()]
}
fn lcs(a: &[String], b: &[String]) -> usize {
    let mut prev = vec![0usize; b.len() + 1];
    for x in a {
        let mut row = vec![0usize; b.len() + 1];
        for (j, y) in b.iter().enumerate() {
            row[j + 1] = if x == y {
                prev[j] + 1
            } else {
                row[j].max(prev[j + 1])
            };
        }
        prev = row;
    }
    prev[b.len()]
}
fn measure(scene: Scene, local: &str, remote: &str, text: &str) -> Result<Value> {
    let local = if scene.local() {
        words(local)
    } else {
        Vec::new()
    };
    let remote = if scene.remote() {
        words(remote)
    } else {
        Vec::new()
    };
    let hypothesis = words(text);
    if local.len().max(remote.len()).max(hypothesis.len()) > 2000 {
        return Err("transcript exceeds 2000-word bound for <=60s trial".into());
    }
    if scene.local() && local.is_empty() {
        return Err("local reference has no words".into());
    }
    if scene.remote() && remote.is_empty() {
        return Err("remote reference has no words".into());
    }
    let local_vocabulary: BTreeSet<_> = local.iter().collect();
    let exclusive: Vec<_> = remote
        .iter()
        .filter(|w| !local_vocabulary.contains(w))
        .cloned()
        .collect();
    let ambiguous: Vec<_> = remote
        .iter()
        .filter(|w| local_vocabulary.contains(w))
        .collect();
    let overlap = lcs(&exclusive, &hypothesis);
    let e = edits(&local, &hypothesis);
    Ok(
        json!({"hypothesis_words": hypothesis.len(), "local_reference_words": local.len(),
        "local_edits": if local.is_empty() { None } else { Some(e) },
        "local_wer": if local.is_empty() { None } else { Some(e.total() as f64/local.len() as f64) },
        "local_deleted_words": if local.is_empty() { None } else { Some(e.deletions) },
        "local_deleted_fraction": if local.is_empty() { None } else { Some(e.deletions as f64/local.len() as f64) },
        "remote_exclusive_words": exclusive.len(), "remote_ambiguous_words": ambiguous.len(),
        "remote_overlap_words_proxy": if remote.is_empty() { None } else { Some(overlap) },
        "remote_overlap_fraction_proxy": if exclusive.is_empty() { None } else { Some(overlap as f64/exclusive.len() as f64) },
        "definition": "WER=Levenshtein S+D+I / local gold words, raw Me includes remote insertions; remote proxy=ordered overlap with remote gold excluding ALL words also in local gold. Not speaker attribution; inspect audio. No denominator => null."}),
    )
}
#[derive(Deserialize)]
struct Asr {
    text: String,
    audio_seconds: f64,
    engine: String,
    model: String,
    backend: String,
    runs: Vec<Value>,
}
fn asr(trial: &Path, track: Track, engine: &str, manifest: &Value) -> Result<Asr> {
    let stem = track.stem();
    let file = track.filename();
    if manifest["files"][&file].as_str() != Some(&protocol::hash(&trial.join(&file))?) {
        return Err(format!("{file}: missing/mismatched manifest hash").into());
    }
    let result: Asr =
        serde_json::from_slice(&fs::read(trial.join(format!("{stem}.{engine}.json")))?)?;
    let wav = hound::WavReader::open(trial.join(file))?;
    let seconds = f64::from(wav.duration()) / f64::from(wav.spec().sample_rate);
    if result.engine != engine
        || result.model.is_empty()
        || result.backend.is_empty()
        || result.runs.is_empty()
        || !result.audio_seconds.is_finite()
        || (result.audio_seconds - seconds).abs() > 0.05
    {
        return Err("ASR metadata inconsistent with recorded WAV / requested profile".into());
    }
    Ok(result)
}
pub fn score(trial: &Path, engine: &str) -> Result<()> {
    if engine != WHISPER_ENGINE_ID && engine != PARAKEET_ENGINE_ID {
        return Err("this protocol requires whisper then parakeet catalogue profiles".into());
    }
    let manifest: Value = serde_json::from_slice(&fs::read(trial.join("manifest.json"))?)?;
    if manifest["valid"] != true
        || !serde_json::from_value::<TrialStatus>(manifest["status"].clone())?.complete()
    {
        return Err("cannot score failed or incomplete capture".into());
    }
    let protocol: Protocol = serde_json::from_slice(&fs::read(trial.join("protocol.json"))?)?;
    protocol.validate()?;
    if manifest["protocol_sha256"].as_str() != Some(&protocol::hash(&trial.join("protocol.json"))?)
    {
        return Err("protocol changed after capture".into());
    }
    let mode: Mode = serde_json::from_value(manifest["mode"].clone())?;
    let scene: Scene = serde_json::from_value(manifest["scene"].clone())?;
    let them = asr(trial, Track::Them, engine, &manifest)?;
    let expected = if engine == WHISPER_ENGINE_ID {
        &protocol.whisper_profile
    } else {
        &protocol.parakeet_profile
    };
    if them.model != expected.model || them.backend != expected.backend {
        return Err("ASR differs from fixed protocol profile".into());
    }
    let mut outputs = BTreeMap::new();
    for &variant in mode.me_tracks() {
        let me = asr(trial, variant, engine, &manifest)?;
        if me.model != them.model || me.backend != them.backend {
            return Err("Me/Them must use exactly the same model and backend".into());
        }
        let metrics = measure(
            scene,
            &protocol.local_reference,
            &protocol.remote_reference,
            &me.text,
        )?;
        outputs.insert(variant.stem(), metrics);
    }
    let remote = words(&protocol.remote_reference);
    if scene.remote() && remote.is_empty() {
        return Err("remote reference has no normalized words".into());
    }
    let them_edits = if scene.remote() {
        Some(edits(&remote, &words(&them.text)))
    } else {
        None
    };
    let score = json!({"engine": engine, "model": them.model, "backend": them.backend,
        "profile_record": if engine == WHISPER_ENGINE_ID { &protocol.whisper_profile } else { &protocol.parakeet_profile },
        "scene": scene, "repetition": manifest["repetition"], "positive_control": manifest["positive_control"],
        "normalization": "lowercase, punctuation removed, whitespace split, accents retained",
        "outputs": outputs, "them_reference_edits": them_edits,
        "them_reference_wer": them_edits.map(|e| e.total() as f64/remote.len() as f64),
        "asr_sha256": protocol::hash(&trial.join(format!("asr.{engine}.json")))?, "protocol_sha256": manifest["protocol_sha256"], "manifest_sha256": protocol::hash(&trial.join("manifest.json"))?});
    protocol::write_json(&trial.join(format!("scores.{engine}.json")), &score)?;
    println!("{}", serde_json::to_string_pretty(&score)?);
    Ok(())
}
fn summary(values: &mut [f64]) -> Value {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if n == 0 {
        return Value::Null;
    }
    json!({"n": n, "median": (values[(n-1)/2]+values[n/2])/2.0, "min": values[0], "max": values[n-1]})
}
pub fn report(directory: &Path) -> Result<()> {
    let mut rows = Vec::new();
    let mut missing = Vec::new();
    let mut groups: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut profiles = BTreeSet::new();
    let mut protocols = BTreeSet::new();
    for mode in [Mode::Baseline, Mode::Apple] {
        for scene in [Scene::Remote, Scene::Local, Scene::Double] {
            for rep in 1..=3 {
                for engine in [WHISPER_ENGINE_ID, PARAKEET_ENGINE_ID] {
                    let trial = directory.join(format!("{}-{}-{rep}", mode.name(), scene.name()));
                    let file = trial.join(format!("scores.{engine}.json"));
                    if !file.exists() {
                        missing.push(file);
                        continue;
                    }
                    let row: Value = serde_json::from_slice(&fs::read(&file)?)?;
                    if row["manifest_sha256"].as_str()
                        != Some(&protocol::hash(&trial.join("manifest.json"))?)
                    {
                        return Err("stale score: manifest changed; rerun score".into());
                    }
                    let manifest: Value =
                        serde_json::from_slice(&fs::read(trial.join("manifest.json"))?)?;
                    if manifest["comparison_eligible"] != true {
                        return Err("auxiliary/headphone/invalid trial cannot enter the 27-variant comparison".into());
                    }
                    if row["asr_sha256"].as_str()
                        != Some(&protocol::hash(&trial.join(format!("asr.{engine}.json")))?)
                    {
                        return Err("ASR provenance changed; rerun score".into());
                    }
                    for track in mode.asr_tracks() {
                        asr(&trial, track, engine, &manifest)?;
                    }
                    protocols.insert(row["protocol_sha256"].to_string());
                    profiles.insert(format!(
                        "{} {} {} {}",
                        row["engine"], row["model"], row["backend"], row["profile_record"]
                    ));
                    let outputs = row["outputs"].as_object().ok_or("missing score outputs")?;
                    for (variant, scores) in outputs {
                        for metric in [
                            "local_wer",
                            "local_deleted_fraction",
                            "remote_overlap_fraction_proxy",
                        ] {
                            if let Some(n) = scores[metric].as_f64() {
                                groups
                                    .entry(format!("{engine}/{}/{variant}/{metric}", scene.name()))
                                    .or_default()
                                    .push(n);
                            }
                        }
                    }
                    rows.push(json!({"trial": trial, "scores": row}));
                }
            }
        }
    }
    // Two fixed engine profiles across every comparison; never silently mix weights.
    if protocols.len() > 1 {
        return Err("protocol differs across comparison trials".into());
    }
    if profiles.len() > 2 {
        return Err(
            "inconsistent ASR profiles across trials; compare identical weights/backend".into(),
        );
    }
    let groups: BTreeMap<_, _> = groups
        .into_iter()
        .map(|(k, mut v)| (k, summary(&mut v)))
        .collect();
    let result = json!({"complete": missing.is_empty(), "missing": missing, "rows": rows, "median_and_range": groups,
        "physical_qa": "pending operator listening, native positive control and coexistence evidence; numeric completeness alone does not satisfy AC1/3/4/6",
        "interpretation": "No default activation recommendation; remote overlap is a lexical proxy and local WER includes remote insertions."});
    protocol::write_json(&directory.join("comparison.json"), &result)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
/// ASR stays in the established headless CLI/catalogue; this only fixes its
/// arguments, checks exit status and binds the JSON to input/output hashes.
pub fn transcribe(trial: &Path, binary: &Path, engine: &str) -> Result<()> {
    if engine != WHISPER_ENGINE_ID && engine != PARAKEET_ENGINE_ID {
        return Err("use whisper or parakeet".into());
    }
    let manifest: Value = serde_json::from_slice(&fs::read(trial.join("manifest.json"))?)?;
    if manifest["valid"] != true
        || !serde_json::from_value::<TrialStatus>(manifest["status"].clone())?.complete()
    {
        return Err("cannot transcribe invalid/incomplete trial".into());
    }
    let p: Protocol = serde_json::from_slice(&fs::read(trial.join("protocol.json"))?)?;
    p.validate()?;
    let profile = if engine == WHISPER_ENGINE_ID {
        &p.whisper_profile
    } else {
        &p.parakeet_profile
    };
    let mode: Mode = serde_json::from_value(manifest["mode"].clone())?;

    let executable = fs::canonicalize(binary)?;
    let mut provenance = json!({"binary": executable, "binary_sha256": protocol::hash(&executable)?, "profile": profile, "artifacts": {}});
    // A crash / failed subprocess leaves no successful provenance marker.
    let marker = trial.join(format!("asr.{engine}.json"));
    if marker.exists() {
        return Err("ASR already exists; retain prior runs and use score, or archive ASR JSON before rerunning".into());
    }
    for track in mode.asr_tracks() {
        let stem = track.stem();
        let wav = fs::canonicalize(trial.join(track.filename()))?;
        let wav_hash = protocol::hash(&wav)?;
        if manifest["files"][track.filename()].as_str() != Some(&wav_hash) {
            return Err("WAV changed since capture".into());
        }
        let output = std::process::Command::new(&executable)
            .arg("--transcribe-file")
            .arg(&wav)
            .args([
                "--engine",
                engine,
                "--model",
                &profile.model,
                "--backend",
                &profile.backend,
                "--json",
            ])
            .output()?;
        fs::write(
            trial.join(format!("{stem}.{engine}.stderr.txt")),
            &output.stderr,
        )?;
        if !output.status.success() {
            return Err(format!(
                "ASR {stem}/{engine} failed: {}; see stderr file",
                output.status
            )
            .into());
        }
        // Check valid CLI schema before publishing any successful result.
        let _: Asr = serde_json::from_slice(&output.stdout)?;
        let target = trial.join(format!("{stem}.{engine}.json"));
        fs::write(&target, output.stdout)?;
        provenance["artifacts"][stem] =
            json!({"wav_sha256": wav_hash, "json_sha256": protocol::hash(&target)?});
    }
    protocol::write_json(&marker, &provenance)?;
    score(trial, engine)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalization_preserves_accents_and_removes_punctuation() {
        assert_eq!(words("L'ÉTÉ, a-t-il fini ?"), vec!["lété", "atil", "fini"]);
        assert_eq!(words("E\u{301}TÉ"), words("été"));
    }
    #[test]
    fn empty_hypothesis_deletes_all_local_words() {
        let v = measure(Scene::Local, "bonjour monde", "remote text", "").unwrap();
        assert_eq!(v["local_wer"], 1.0);
        assert_eq!(v["local_deleted_words"], 2);
        assert!(v["remote_overlap_fraction_proxy"].is_null());
    }
    #[test]
    fn remote_only_has_no_local_denominator() {
        let v = measure(Scene::Remote, "hello", "remote words", "").unwrap();
        assert!(v["local_wer"].is_null());
        assert_eq!(v["remote_overlap_fraction_proxy"], 0.0);
    }
    #[test]
    fn shared_words_are_not_counted_as_remote_duplicates() {
        let v = measure(
            Scene::Double,
            "the local voice",
            "the distant voice",
            "the local voice",
        )
        .unwrap();
        assert_eq!(v["remote_ambiguous_words"], 2);
        assert_eq!(v["remote_overlap_words_proxy"], 0);
    }
    #[test]
    fn substitution_deletion_insertion_are_separate() {
        assert_eq!(
            edits(&words("one two three"), &words("one four")),
            Edits {
                substitutions: 1,
                deletions: 1,
                insertions: 0
            }
        );
    }
    #[test]
    fn missing_trials_never_report_complete() {
        let dir = tempfile::tempdir().unwrap();
        report(dir.path()).unwrap();
        let v: Value =
            serde_json::from_slice(&fs::read(dir.path().join("comparison.json")).unwrap()).unwrap();
        assert_eq!(v["complete"], false);
        assert_eq!(v["missing"].as_array().unwrap().len(), 36);
    }
    #[test]
    fn missing_asr_is_an_error_not_a_perfect_score() {
        let dir = tempfile::tempdir().unwrap();
        assert!(score(dir.path(), "whisper").is_err());
    }
    #[test]
    fn medians_and_range_retain_bad_run() {
        assert_eq!(
            summary(&mut [0.0, 0.1, 1.0]),
            json!({"n":3,"median":0.1,"min":0.0,"max":1.0})
        );
    }
}
