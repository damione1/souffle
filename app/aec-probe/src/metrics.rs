//! Text metrics are ASR observations, not acoustic speaker attribution.
//! Shared gold words are ambiguous and excluded from remote-overlap counts.
use crate::{
    Result,
    protocol::{self, AsrProfile, Mode, Protocol, Scene, Track, TrialStatus},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use souffle_lib::engine::{PARAKEET_ENGINE_ID, WHISPER_ENGINE_ID};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
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
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AsrArtifact {
    wav_sha256: String,
    json_sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AsrProvenance {
    binary: PathBuf,
    binary_sha256: String,
    profile: AsrProfile,
    artifacts: BTreeMap<String, AsrArtifact>,
}
fn trial_protocol(trial: &Path, manifest: &Value) -> Result<Protocol> {
    let path = trial.join("protocol.json");
    if manifest["protocol_sha256"].as_str() != Some(&protocol::hash(&path)?) {
        return Err("protocol changed after capture".into());
    }
    let protocol: Protocol = serde_json::from_slice(&fs::read(path)?)?;
    protocol.validate()?;
    Ok(protocol)
}
fn profile<'a>(protocol: &'a Protocol, engine: &str) -> Result<&'a AsrProfile> {
    match engine {
        WHISPER_ENGINE_ID => Ok(&protocol.whisper_profile),
        PARAKEET_ENGINE_ID => Ok(&protocol.parakeet_profile),
        _ => Err("this protocol requires whisper then parakeet catalogue profiles".into()),
    }
}
fn asr_metadata(bytes: &[u8], wav: &Path, engine: &str, profile: &AsrProfile) -> Result<Asr> {
    let result: Asr = serde_json::from_slice(bytes)?;
    let wav = hound::WavReader::open(wav)?;
    let seconds = f64::from(wav.duration()) / f64::from(wav.spec().sample_rate);
    if result.engine != engine
        || result.model != profile.model
        || result.backend != profile.backend
        || result.runs.is_empty()
        || !result.audio_seconds.is_finite()
        || (result.audio_seconds - seconds).abs() > 0.05
    {
        return Err("ASR metadata inconsistent with recorded WAV / requested profile".into());
    }
    Ok(result)
}
fn asr(
    trial: &Path,
    track: Track,
    engine: &str,
    manifest: &Value,
    profile: &AsrProfile,
) -> Result<Asr> {
    let provenance: AsrProvenance =
        serde_json::from_slice(&fs::read(trial.join(format!("asr.{engine}.json")))?)?;
    // The executable identity is historical evidence; reports remain usable if
    // the operator later moves or updates that binary. Never imply a fresh hash.
    if provenance.profile != *profile
        || !provenance.binary.is_absolute()
        || provenance.binary_sha256.len() != 64
        || !provenance
            .binary_sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err("invalid ASR provenance / fixed protocol profile".into());
    }
    let stem = track.stem();
    let file = track.filename();
    let wav_hash = protocol::hash(&trial.join(&file))?;
    if manifest["files"][&file].as_str() != Some(&wav_hash) {
        return Err(format!("{file}: missing/mismatched manifest hash").into());
    }
    let artifact = provenance
        .artifacts
        .get(stem)
        .ok_or("missing ASR artifact provenance")?;
    let target = trial.join(format!("{stem}.{engine}.json"));
    if artifact.wav_sha256 != wav_hash || artifact.json_sha256 != protocol::hash(&target)? {
        return Err(format!(
            "{stem}/{engine}: ASR artifact changed since successful transcription"
        )
        .into());
    }
    asr_metadata(&fs::read(target)?, &trial.join(file), engine, profile)
}
pub fn score(trial: &Path, engine: &str) -> Result<()> {
    let manifest: Value = serde_json::from_slice(&fs::read(trial.join("manifest.json"))?)?;
    if manifest["valid"] != true
        || !serde_json::from_value::<TrialStatus>(manifest["status"].clone())?.complete()
    {
        return Err("cannot score failed or incomplete capture".into());
    }
    let protocol = trial_protocol(trial, &manifest)?;
    let expected = profile(&protocol, engine)?;
    let mode: Mode = serde_json::from_value(manifest["mode"].clone())?;
    let scene: Scene = serde_json::from_value(manifest["scene"].clone())?;
    let them = asr(trial, Track::Them, engine, &manifest, expected)?;
    let mut outputs = BTreeMap::new();
    for &variant in mode.me_tracks() {
        let me = asr(trial, variant, engine, &manifest, expected)?;
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
        "profile_record": expected,
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
                    let protocol = trial_protocol(&trial, &manifest)?;
                    if row["protocol_sha256"] != manifest["protocol_sha256"] {
                        return Err("stale score: protocol hash differs; rerun score".into());
                    }
                    if manifest["comparison_eligible"] != true {
                        return Err("auxiliary/headphone/invalid trial cannot enter the 27-variant comparison".into());
                    }
                    if row["asr_sha256"].as_str()
                        != Some(&protocol::hash(&trial.join(format!("asr.{engine}.json")))?)
                    {
                        return Err("ASR provenance changed; rerun score".into());
                    }
                    for track in mode.asr_tracks() {
                        asr(
                            &trial,
                            track,
                            engine,
                            &manifest,
                            profile(&protocol, engine)?,
                        )?;
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
    let manifest: Value = serde_json::from_slice(&fs::read(trial.join("manifest.json"))?)?;
    if manifest["valid"] != true
        || !serde_json::from_value::<TrialStatus>(manifest["status"].clone())?.complete()
    {
        return Err("cannot transcribe invalid/incomplete trial".into());
    }
    let p = trial_protocol(trial, &manifest)?;
    let profile = profile(&p, engine)?;
    let mode: Mode = serde_json::from_value(manifest["mode"].clone())?;

    let executable = fs::canonicalize(binary)?;
    let mut provenance = AsrProvenance {
        binary: executable.clone(),
        binary_sha256: protocol::hash(&executable)?,
        profile: profile.clone(),
        artifacts: BTreeMap::new(),
    };
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
        asr_metadata(&output.stdout, &wav, engine, profile)?;
        let target = trial.join(format!("{stem}.{engine}.json"));
        fs::write(&target, output.stdout)?;
        provenance.artifacts.insert(
            stem.into(),
            AsrArtifact {
                wav_sha256: wav_hash,
                json_sha256: protocol::hash(&target)?,
            },
        );
    }
    protocol::write_json(&marker, &provenance)?;
    score(trial, engine)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scored_trial() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let trial = directory.path().join("baseline-remote-1");
        fs::create_dir(&trial).unwrap();
        let profile = AsrProfile {
            model: "synthetic-model".into(),
            backend: "synthetic-backend".into(),
            weights_sha256: "a".repeat(64),
        };
        let protocol = Protocol {
            fixture: "synthetic.wav".into(),
            fixture_sha256: "b".repeat(64),
            remote_reference: "distant words".into(),
            local_reference: "nearby voice".into(),
            standard_microphone_mode_confirmed: true,
            machine_position: "synthetic test".into(),
            mouth_distance_cm: 50.0,
            output_volume_description: "synthetic".into(),
            browser_version: "synthetic".into(),
            schedule: "synthetic".into(),
            whisper_profile: profile.clone(),
            parakeet_profile: profile.clone(),
        };
        protocol::write_json(&trial.join("protocol.json"), &protocol).unwrap();
        let mut provenance = AsrProvenance {
            binary: PathBuf::from("/synthetic/souffle"),
            binary_sha256: "c".repeat(64),
            profile: profile.clone(),
            artifacts: BTreeMap::new(),
        };
        let mut files = BTreeMap::new();
        for track in Mode::Baseline.asr_tracks() {
            let wav = trial.join(track.filename());
            protocol::write_wav(&wav, &[0.1; 800], 8000).unwrap();
            let target = trial.join(format!("{}.whisper.json", track.stem()));
            protocol::write_json(
                &target,
                &json!({
                    "text": "distant words", "audio_seconds": 0.1,
                    "engine": WHISPER_ENGINE_ID, "model": profile.model,
                    "backend": profile.backend, "runs": [{}],
                }),
            )
            .unwrap();
            let wav_sha256 = protocol::hash(&wav).unwrap();
            files.insert(track.filename(), wav_sha256.clone());
            provenance.artifacts.insert(
                track.stem().into(),
                AsrArtifact {
                    wav_sha256,
                    json_sha256: protocol::hash(&target).unwrap(),
                },
            );
        }
        protocol::write_json(
            &trial.join("manifest.json"),
            &json!({
                "valid": true, "status": TrialStatus::Complete, "mode": Mode::Baseline,
                "scene": Scene::Remote, "repetition": 1, "positive_control": false,
                "comparison_eligible": true, "files": files,
                "protocol_sha256": protocol::hash(&trial.join("protocol.json")).unwrap(),
            }),
        )
        .unwrap();
        protocol::write_json(&trial.join("asr.whisper.json"), &provenance).unwrap();
        score(&trial, WHISPER_ENGINE_ID).unwrap();
        report(directory.path()).unwrap();
        (directory, trial)
    }

    #[test]
    fn replaced_track_json_is_rejected_by_score_and_report() {
        let (directory, trial) = scored_trial();
        for track in Mode::Baseline.asr_tracks() {
            let target = trial.join(format!("{}.whisper.json", track.stem()));
            let original = fs::read(&target).unwrap();
            let mut edited: Value = serde_json::from_slice(&original).unwrap();
            edited["text"] = json!("replacement transcript with unchanged metadata");
            protocol::write_json(&target, &edited).unwrap();
            for result in [score(&trial, WHISPER_ENGINE_ID), report(directory.path())] {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("ASR artifact changed")
                );
            }
            fs::write(target, original).unwrap();
        }
        report(directory.path()).unwrap();
    }

    #[test]
    fn changed_protocol_invalidates_existing_scores_and_report() {
        let (directory, trial) = scored_trial();
        let path = trial.join("protocol.json");
        let mut protocol: Protocol = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        protocol.remote_reference = "changed reference".into();
        protocol::write_json(&path, &protocol).unwrap();
        for result in [score(&trial, WHISPER_ENGINE_ID), report(directory.path())] {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("protocol changed after capture")
            );
        }
    }

    #[test]
    fn provenance_requires_matching_wav_profile_and_binary_identity() {
        let (_directory, trial) = scored_trial();
        let marker = trial.join("asr.whisper.json");
        let original = fs::read(&marker).unwrap();
        for replacement in [
            json!({"profile": {"model": "other-model"}}),
            json!({"binary_sha256": "missing hash"}),
            json!({"binary": "relative/path"}),
            json!({"artifacts": {"them": {"wav_sha256": "d".repeat(64)}}}),
        ] {
            let mut provenance: Value = serde_json::from_slice(&original).unwrap();
            // Mutate just the selected leaf, retaining the otherwise valid marker.
            match replacement.as_object().unwrap().iter().next().unwrap() {
                (key, value) if key == "profile" => {
                    provenance[key]["model"] = value["model"].clone()
                }
                (key, value) if key == "artifacts" => {
                    provenance[key]["them"]["wav_sha256"] = value["them"]["wav_sha256"].clone()
                }
                (key, value) => provenance[key] = value.clone(),
            }
            protocol::write_json(&marker, &provenance).unwrap();
            assert!(score(&trial, WHISPER_ENGINE_ID).is_err());
        }
        fs::remove_file(marker).unwrap();
        assert!(score(&trial, WHISPER_ENGINE_ID).is_err());
    }

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
