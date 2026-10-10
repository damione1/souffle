// Private, opt-in benchmark of the unmodified production summary pipeline.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use souffle_lib::engine::{Speaker, TranscriptionSegment};
use souffle_lib::summary::{self, SummaryLanguage};
use souffle_lib::transcript::{MeetingParticipant, StructuredSummary};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

#[derive(Clone, Serialize, Deserialize)]
struct Input {
    id: String,
    title: String,
    duration_seconds: f64,
    segments: Vec<TranscriptionSegment>,
    edited_transcript: Option<String>,
    notes: Option<String>,
    participants: Vec<MeetingParticipant>,
    #[serde(default = "french")]
    language: SummaryLanguage,
    #[serde(default = "default_template")]
    template: String,
}
fn french() -> SummaryLanguage {
    SummaryLanguage::Fr
}
fn default_template() -> String {
    summary::TEMPLATE_SUMMARY_DEFAULT.into()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Third {
    Beginning,
    Middle,
    End,
}
#[derive(Serialize, Deserialize)]
struct Fact {
    id: String,
    text: String,
    third: Third,
    /// Each group must match; alternatives within one group may match.
    keywords: Vec<Vec<String>>,
    owner: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Gold {
    meeting_id: String,
    corpus_sha256: String,
    validated_by: Option<String>,
    validated_at: Option<String>,
    facts: Vec<Fact>,
}
#[derive(Default, Serialize, Deserialize)]
struct Review {
    /// Override keyword false positives/negatives after reading this exact run.
    matches: BTreeMap<String, bool>,
    /// None means NOT reviewed, never zero hallucinations.
    invented_facts: Option<usize>,
    reviewed_by: Option<String>,
    run_sha256: Option<String>,
}
#[derive(Debug, Serialize)]
struct Score {
    matched: usize,
    total: usize,
    by_third: BTreeMap<Third, (usize, usize)>,
    owners_preserved: usize,
    owners_total: usize,
    duplicated_headings: usize,
    invented_facts: Option<usize>,
}
fn fold(s: &str) -> String {
    s.nfd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn contains_phrase(haystack: &str, needle: &str) -> bool {
    !needle.is_empty() && format!(" {haystack} ").contains(&format!(" {needle} "))
}
fn score(prose: &str, structured: &StructuredSummary, gold: &Gold, review: &Review) -> Score {
    let structured_text = serde_json::to_string(structured).unwrap();
    let text = fold(&format!("{prose}\n{structured_text}"));
    let mut result = Score {
        matched: 0,
        total: gold.facts.len(),
        by_third: BTreeMap::new(),
        owners_preserved: 0,
        owners_total: 0,
        duplicated_headings: 0,
        invented_facts: review.invented_facts,
    };
    for fact in &gold.facts {
        let matched = review.matches.get(&fact.id).copied().unwrap_or_else(|| {
            !fact.keywords.is_empty()
                && fact.keywords.iter().all(|group| {
                    group
                        .iter()
                        .any(|keyword| contains_phrase(&text, &fold(keyword)))
                })
        });
        result.matched += usize::from(matched);
        let entry = result.by_third.entry(fact.third).or_default();
        entry.0 += usize::from(matched);
        entry.1 += 1;
        if let Some(owner) = &fact.owner {
            result.owners_total += 1;
            // An owner's name anywhere in the document is not an attribution.
            result.owners_preserved += usize::from(
                matched
                    && structured.action_items.iter().any(|action| {
                        action
                            .owner
                            .as_ref()
                            .is_some_and(|actual| fold(actual) == fold(owner))
                            && !fact.keywords.is_empty()
                            && fact.keywords.iter().all(|group| {
                                group.iter().any(|keyword| {
                                    contains_phrase(&fold(&action.text), &fold(keyword))
                                })
                            })
                    }),
            );
        }
    }
    for heading in ["summary", "topics"] {
        result.duplicated_headings += prose
            .lines()
            .filter(|line| {
                line.trim()
                    .strip_prefix("## ")
                    .is_some_and(|title| fold(title) == heading)
            })
            .count()
            .saturating_sub(1);
    }
    result
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> T {
    serde_json::from_slice(&std::fs::read(path).expect("read private JSON")).expect("valid JSON")
}
fn write_json(path: &Path, value: &impl Serialize) {
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).expect("write private report");
}
fn outside_repo(path: &Path) -> PathBuf {
    assert!(path.is_absolute(), "benchmark directories must be absolute");
    std::fs::create_dir_all(path).expect("create benchmark directory");
    let path = path.canonicalize().unwrap();
    // Resolve symlinks before checking: outputs and private corpus must never land in Git.
    // A linked worktree has a .git FILE, an ordinary checkout a directory.
    // Check every ancestor so this also rejects sibling/unrelated checkouts.
    assert!(
        path.ancestors()
            .all(|ancestor| !ancestor.join(".git").exists()),
        "private benchmark data must stay outside every Git checkout"
    );
    path
}
fn command(program: &str, args: &[&str]) -> String {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .expect("metadata command");
    assert!(
        output.status.success(),
        "metadata command failed: {program}"
    );
    String::from_utf8_lossy(&output.stdout).trim().into()
}
fn db_inputs(path: &Path, ids: &[String]) -> Vec<Input> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open DB read-only; no migration");
    ids.iter().map(|id| {
        let (title, duration_seconds, edited_transcript, notes, participants): (String, f64, Option<String>, Option<String>, String) = db.query_row(
            "SELECT title, duration_seconds, edited_transcript, notes, participants FROM meetings WHERE id=?1", [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).expect("selected meeting");
        let mut stmt = db.prepare("SELECT text,start_time,end_time,is_final,language,confidence,speaker FROM segments WHERE meeting_id=?1 ORDER BY sort_order").unwrap();
        let segments = stmt.query_map([id], |r| Ok(TranscriptionSegment {
            text: r.get(0)?, start_time: r.get(1)?, end_time: r.get(2)?, is_final: r.get::<_,i32>(3)? != 0,
            language: r.get(4)?, confidence: r.get(5)?, speaker: r.get::<_,Option<String>>(6)?.as_deref().and_then(Speaker::parse),
        })).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        Input { id: id.clone(), title, duration_seconds, segments, edited_transcript, notes, participants: serde_json::from_str(&participants).expect("participants"), language: french(), template: default_template() }
    }).collect()
}

#[derive(Serialize, Deserialize)]
struct Run {
    meeting_id: String,
    model: String,
    repetition: usize,
    metadata: serde_json::Value,
    prose: Option<String>,
    structured: Option<StructuredSummary>,
    diagnostics: serde_json::Value,
    elapsed_ms: u128,
    error: Option<String>,
}

/// Rescoring is offline: the saved artifacts, not today's device, choose
/// providers. Return per-meeting sets so partially measured corpora are honest.
fn saved_models(output_dir: &Path, meeting_id: &str) -> Vec<String> {
    let mut models = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(output_dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name().to_string_lossy().ends_with(".run.json") {
            let run: Run = read_json(&entry.path());
            if run.meeting_id == meeting_id {
                models.insert(run.model);
            }
        }
    }
    assert!(!models.is_empty(), "no saved runs for meeting {meeting_id}");
    models.into_iter().collect()
}

struct AggregateSample {
    recall: f64,
    elapsed_ms: u128,
    calls: usize,
}

// Integration tests exercise the shared scoring helpers; only the normal
// example entry executes private provider runs and dispatches Apple children.
#[cfg_attr(test, allow(dead_code))]
pub(crate) async fn run_bench() {
    let corpus_dir = outside_repo(&PathBuf::from(
        std::env::var("SOUFFLE_BENCH_CORPUS_DIR").expect("SOUFFLE_BENCH_CORPUS_DIR"),
    ));
    let output_dir = outside_repo(&PathBuf::from(
        std::env::var("SOUFFLE_BENCH_OUTPUT_DIR").expect("SOUFFLE_BENCH_OUTPUT_DIR"),
    ));
    let mode = std::env::var("SOUFFLE_BENCH_MODE").unwrap_or_else(|_| "run".into());
    assert!(
        matches!(mode.as_str(), "prepare" | "run" | "score"),
        "unknown benchmark mode"
    );
    if mode == "prepare" {
        let ids: Vec<String> = std::env::var("SOUFFLE_BENCH_MEETING_IDS")
            .expect("explicit meeting IDs")
            .split(',')
            .map(str::to_string)
            .collect();
        let inputs = db_inputs(
            Path::new(&std::env::var("SOUFFLE_BENCH_DB").expect("explicit read-only database")),
            &ids,
        );
        for input in inputs {
            let path = corpus_dir.join(format!("{}.json", input.id));
            assert!(
                !path.exists(),
                "prepare never overwrites an existing corpus"
            );
            write_json(&path, &input);
            let gold_path = corpus_dir.join(format!("{}.gold.json", input.id));
            assert!(!gold_path.exists(), "prepare never overwrites a gold");
            write_json(
                &gold_path,
                &Gold {
                    meeting_id: input.id,
                    corpus_sha256: hash(&std::fs::read(path).unwrap()),
                    validated_by: None,
                    validated_at: None,
                    facts: vec![],
                },
            );
        }
        return;
    }
    let mut paths: Vec<_> = std::fs::read_dir(&corpus_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.extension().is_some_and(|ext| ext == "json")
                && !p
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .ends_with(".gold.json")
        })
        .collect();
    paths.sort();
    assert!(paths.len() >= 3, "at least three private meetings required");
    let mut corpus = Vec::new();
    for path in paths {
        let input: Input = read_json(&path);
        assert!(
            !input.id.is_empty()
                && input
                    .id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "meeting IDs must be safe file names"
        );
        assert_eq!(
            input.language,
            SummaryLanguage::Fr,
            "baseline corpus must be French"
        );
        let gold_path = corpus_dir.join(format!("{}.gold.json", input.id));
        let gold: Gold = read_json(&gold_path);
        assert_eq!(gold.meeting_id, input.id);
        assert_eq!(gold.corpus_sha256, hash(&std::fs::read(&path).unwrap()));
        assert!(
            (15..=25).contains(&gold.facts.len()),
            "prepare 15–25 independent gold facts"
        );
        assert!(
            gold.validated_by
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty())
                && gold
                    .validated_at
                    .as_ref()
                    .is_some_and(|s| !s.trim().is_empty()),
            "Damien must validate the gold BEFORE quality runs"
        );
        let mut ids = std::collections::BTreeSet::new();
        for fact in &gold.facts {
            assert!(
                !fact.text.trim().is_empty() && !fact.keywords.is_empty() && ids.insert(&fact.id)
            );
            assert!(
                fact.keywords
                    .iter()
                    .all(|g| !g.is_empty() && g.iter().all(|s| !fold(s).is_empty()))
            );
        }
        corpus.push((
            input,
            gold,
            hash(&std::fs::read(gold_path).unwrap()),
            hash(&std::fs::read(path).unwrap()),
        ));
    }
    for (lower, upper) in [(25.0, 35.0), (40.0, 50.0), (65.0, 85.0)] {
        assert!(
            corpus
                .iter()
                .any(|(input, _, _, _)| (lower..=upper).contains(&(input.duration_seconds / 60.0))),
            "corpus must include representative 30/45/75 minute meetings"
        );
    }
    let mut table = String::from(
        "# Summary benchmark\n\nKeyword recall is provisional until human correction; unreviewed inventions are unknown. Invalid runs have no quality score.\n\n| Meeting | Model | Run | Recall | Beginning / middle / end | Owners | Duplicate headings | Calls | Tokens in/out | ms | Merge rounds | Invented facts |\n|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    let url = std::env::var("SOUFFLE_BENCH_OLLAMA_URL")
        .unwrap_or_else(|_| "http://localhost:11434".into());
    let commit = command("git", &["rev-parse", "HEAD"]);
    let machine = command(
        "sysctl",
        &["-n", "hw.model", "hw.memsize", "machdep.cpu.brand_string"],
    );
    let os = command("sw_vers", &[]);
    let client = reqwest::Client::new();
    let version = if mode == "run" {
        client
            .get(format!("{url}/api/version"))
            .send()
            .await
            .ok()
            .and_then(|r| r.error_for_status().ok())
    } else {
        None
    };
    let version = match version {
        Some(r) => r.json::<serde_json::Value>().await.ok(),
        None => None,
    };
    let tags = if mode == "run" {
        client
            .get(format!("{url}/api/tags"))
            .send()
            .await
            .ok()
            .and_then(|r| r.error_for_status().ok())
    } else {
        None
    };
    let tags = match tags {
        Some(r) => r.json::<serde_json::Value>().await.ok(),
        None => None,
    };
    let mut run_models = vec!["qwen2.5:7b".to_string(), "qwen3:4b".to_string()];
    let availability_path = output_dir.join("provider-availability.json");
    if mode == "run" {
        let apple_available = summary::apple_intelligence_available();
        if apple_available {
            run_models.push(summary::APPLE_INTELLIGENCE_MODEL_ID.to_string());
        }
        assert!(
            !availability_path.exists(),
            "choose a new output directory for each measurement campaign"
        );
        write_json(
            &availability_path,
            &serde_json::json!({
                "apple_available": apple_available,
                "apple_unavailable_reason": if apple_available { None } else { Some(format!("{:?}", souffle_lib::apple_intelligence::unavailable_reason())) },
                "apple_stub": souffle_lib::apple_intelligence::is_stub_linked(),
            }),
        );
    }
    if availability_path.exists() {
        let provenance: serde_json::Value = read_json(&availability_path);
        if provenance["apple_available"] == false {
            table.push_str(&format!("\nApple not measured at run time: {}; stub={}. No Apple merge baseline is claimed.\n\n", provenance["apple_unavailable_reason"], provenance["apple_stub"]));
        }
    } else {
        table.push_str("\nProvider availability was not recorded in these saved artifacts. Only saved models are rescored.\n\n");
    }
    let mut aggregates: BTreeMap<(String, String), Vec<AggregateSample>> = BTreeMap::new();
    // Runs/configurations are sequential; map concurrency stays in production.
    for (input, gold, gold_hash, corpus_hash) in corpus {
        let models = if mode == "score" {
            saved_models(&output_dir, &input.id)
        } else {
            run_models.clone()
        };
        for model in &models {
            for repetition in 1..=3 {
                let stem = format!("{}-{}-{repetition}", input.id, model.replace(':', "-"));
                let path = output_dir.join(format!("{stem}.run.json"));
                if mode == "run" {
                    assert!(
                        !path.exists(),
                        "run files are immutable; choose a new output directory"
                    );
                    let settings = souffle_lib::settings::AppSettings::default();
                    let system =
                        summary::resolve_summary_template_prompt(&settings, Some(&input.template));
                    let turns = summary::turns_from_segments(&input.segments);
                    let edited = input.edited_transcript.as_ref().filter(|s| !s.is_empty());
                    let text = edited.cloned().unwrap_or_else(|| turns.join("\n"));
                    let start = Instant::now();
                    let (result, metrics) = summary::metrics::measure(async {
                        let prose = summary::summarize_stream(
                            &text,
                            edited.is_none().then_some(turns.as_slice()),
                            input.notes.as_deref(),
                            &input.participants,
                            model,
                            Some(&url),
                            &system,
                            input.language,
                            |_| {},
                        )
                        .await?;
                        let structured = summary::extract_structured_summary(
                            &prose,
                            input.notes.as_deref(),
                            &input.participants,
                            model,
                            Some(&url),
                            input.language,
                        )
                        .await;
                        Ok::<_, String>((prose, structured))
                    })
                    .await;
                    let (prose, structured, error) = match result {
                        Ok((prose, Ok(structured))) => (Some(prose), Some(structured), None),
                        Ok((prose, Err(error))) => (Some(prose), None, Some(error)),
                        Err(error) => (None, None, Some(error)),
                    };
                    write_json(
                        &path,
                        &Run {
                            meeting_id: input.id.clone(),
                            model: model.to_string(),
                            repetition,
                            metadata: serde_json::json!({"commit":commit,"machine":machine,"os":os,"corpus_sha256":corpus_hash,"gold_sha256":gold_hash,"gold_validated_by":gold.validated_by,"gold_validated_at":gold.validated_at,"template":input.template,"system_prompt":system,"language":input.language,"ollama_version":version,"ollama_models":tags,"ollama_url":url,"apple_stub":souffle_lib::apple_intelligence::is_stub_linked(),"parameters":"production: map=0.2/retry=0.7 merge=0.2 final=0.3 stuff=0.2 extract=0.1; Apple bridge defaults"}),
                            prose,
                            structured,
                            error,
                            diagnostics: serde_json::to_value(metrics).unwrap(),
                            elapsed_ms: start.elapsed().as_millis(),
                        },
                    );
                }
                let run: Run = read_json(&path);
                assert_eq!(run.meeting_id, input.id);
                assert_eq!(&run.model, model);
                assert_eq!(run.repetition, repetition);
                assert_eq!(
                    run.metadata["gold_sha256"], gold_hash,
                    "score the exact validated gold used by this run"
                );
                assert_eq!(run.metadata["corpus_sha256"], corpus_hash);
                let review_path = output_dir.join(format!("{stem}.review.json"));
                if !review_path.exists() {
                    write_json(
                        &review_path,
                        &Review {
                            run_sha256: Some(hash(&std::fs::read(&path).unwrap())),
                            ..Review::default()
                        },
                    );
                }
                let review: Review = read_json(&review_path);
                assert_eq!(
                    review.run_sha256.as_deref(),
                    Some(hash(&std::fs::read(&path).unwrap()).as_str()),
                    "review must refer to this exact run"
                );
                if review.invented_facts.is_some() || !review.matches.is_empty() {
                    assert!(
                        review
                            .reviewed_by
                            .as_ref()
                            .is_some_and(|s| !s.trim().is_empty()),
                        "manual review requires reviewer provenance"
                    );
                }
                let calls = run.diagnostics["calls"].as_array().unwrap();
                let tokens = |field: &str| -> Option<u64> {
                    calls
                        .iter()
                        .map(|c| c[field].as_u64())
                        .try_fold(0u64, |sum, v| v.map(|v| sum + v))
                };
                match (&run.prose, &run.structured, &run.error) {
                    (Some(prose), Some(structured), None) => {
                        let result = score(prose, structured, &gold, &review);
                        write_json(&output_dir.join(format!("{stem}.score.json")), &result);
                        let thirds = [Third::Beginning,Third::Middle,Third::End].map(|t| { let (m,n)=result.by_third.get(&t).copied().unwrap_or_default(); format!("{m}/{n}") }).join(" / ");
                        table.push_str(&format!("| {} | {} | {} | {}/{} | {} | {}/{} | {} | {} | {:?}/{:?} | {} | {} | {} |\n", input.id,model,repetition,result.matched,result.total,thirds,result.owners_preserved,result.owners_total,result.duplicated_headings,calls.len(),tokens("prompt_tokens"),tokens("output_tokens"),run.elapsed_ms,run.diagnostics["merge_rounds"],result.invented_facts.map(|n| n.to_string()).unwrap_or_else(|| "NOT REVIEWED".into())));
                        aggregates.entry((input.id.clone(),model.to_string())).or_default().push(AggregateSample { recall: result.matched as f64/result.total as f64, elapsed_ms: run.elapsed_ms, calls: calls.len() });
                    }
                    _ => table.push_str(&format!("| {} | {} | {} | INVALID | — | — | — | {} | {:?}/{:?} | {} | {} | NOT REVIEWED |\n",input.id,model,repetition,calls.len(),tokens("prompt_tokens"),tokens("output_tokens"),run.elapsed_ms,run.diagnostics["merge_rounds"])),
                }
            }
        }
    }
    table.push_str("\n## Aggregate over valid runs\n\n| Meeting | Model | Valid runs / 3 | Mean recall | Mean ms | Mean calls |\n|---|---|---|---|---|---|\n");
    for ((meeting, model), values) in aggregates {
        let n = values.len() as f64;
        table.push_str(&format!(
            "| {meeting} | {model} | {} / 3 | {:.3} | {:.0} | {:.2} |\n",
            values.len(),
            values.iter().map(|v| v.recall).sum::<f64>() / n,
            values.iter().map(|v| v.elapsed_ms as f64).sum::<f64>() / n,
            values.iter().map(|v| v.calls as f64).sum::<f64>() / n
        ));
    }
    let date = chrono::Local::now().format("%Y-%m-%d");
    std::fs::write(output_dir.join(format!("bench-resumes-{date}.md")), table).unwrap();
}
