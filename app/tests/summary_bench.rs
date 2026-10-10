//! Private opt-in benchmark wrapper and synthetic scoring/diagnostics tests.
include!("support/summary_bench.rs");

fn benchmark_command() -> std::process::Command {
    let mut command =
        std::process::Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command.args([
        "run",
        "--quiet",
        "--manifest-path",
        concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"),
        "--example",
        "summary_bench",
    ]);
    command
}

#[test]
#[ignore = "private corpus and validated gold required; calls production providers"]
fn summary_bench() {
    // A libtest executable rejects the internal Apple helper argument.
    // Run the benchmark in a normal Rust entry point that dispatches it,
    // preserving the exact isolated-child path used by the shipped app.
    assert!(
        benchmark_command()
            .status()
            .expect("run Rust benchmark")
            .success()
    );
}

#[test]
fn benchmark_entry_dispatches_apple_helper_before_reading_private_corpus() {
    let output = benchmark_command()
        .args(["--", "--internal-apple-intelligence-helper"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run benchmark helper entry");
    // Empty input must produce a framed protocol error, not a libtest
    // unknown-option error, a corpus read, or a FoundationModels request.
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout.first(), Some(&0));
    assert!(output.stdout.len() >= 9);
    let length = u64::from_le_bytes(output.stdout[1..9].try_into().unwrap()) as usize;
    assert_eq!(output.stdout.len(), length + 9);
    let error = std::str::from_utf8(&output.stdout[9..]).unwrap();
    assert!(error.contains("Read Apple Intelligence helper field length"));
}

#[test]
fn keyword_scoring_folds_accents_matches_words_and_keeps_owner_attribution() {
    let gold = Gold {
        meeting_id: "synthetic".into(),
        corpus_sha256: String::new(),
        validated_by: None,
        validated_at: None,
        facts: vec![Fact {
            id: "f1".into(),
            text: "Échéance été".into(),
            third: Third::End,
            keywords: vec![vec!["échéance".into()], vec!["été".into()]],
            owner: Some("Alice".into()),
        }],
    };
    let structured = StructuredSummary {
        decisions: vec![],
        action_items: vec![],
        open_questions: vec![],
    };
    assert_eq!(
        score(
            "Alice: société, échéance",
            &structured,
            &gold,
            &Review::default()
        )
        .matched,
        0
    );
    let result = score(
        "## Summary\n## Summary\n## Topics\nÉCHÉANCE été Alice",
        &structured,
        &gold,
        &Review::default(),
    );
    assert_eq!(
        (
            result.matched,
            result.owners_preserved,
            result.duplicated_headings,
            result.invented_facts
        ),
        (1, 0, 1, None)
    );
    assert_eq!(result.by_third[&Third::End], (1, 1));
    let review = Review {
        matches: BTreeMap::from([("f1".into(), false)]),
        ..Review::default()
    };
    assert_eq!(
        score("échéance été", &structured, &gold, &review).matched,
        0
    );
    let attributed = StructuredSummary {
        action_items: vec![souffle_lib::transcript::StructuredActionItem {
            text: "échéance été".into(),
            owner: Some("Alice".into()),
        }],
        ..structured
    };
    assert_eq!(
        score("échéance été", &attributed, &gold, &Review::default()).owners_preserved,
        1
    );
    let corrected = score("échéance été", &attributed, &gold, &review);
    assert_eq!((corrected.matched, corrected.owners_preserved), (0, 0));
}

#[test]
fn rescore_uses_only_saved_models_for_each_meeting() {
    let output = tempfile::tempdir().unwrap();
    let mut run = Run {
        meeting_id: "ollama-only".into(),
        model: "qwen2.5:7b".into(),
        repetition: 1,
        metadata: serde_json::json!({}),
        prose: None,
        structured: None,
        diagnostics: serde_json::json!({}),
        elapsed_ms: 0,
        error: Some("synthetic invalid run".into()),
    };
    write_json(&output.path().join("one.run.json"), &run);
    assert_eq!(
        saved_models(output.path(), "ollama-only"),
        vec!["qwen2.5:7b"]
    );
    run.meeting_id = "apple-measured".into();
    run.model = summary::APPLE_INTELLIGENCE_MODEL_ID.into();
    write_json(&output.path().join("two.run.json"), &run);
    assert_eq!(
        saved_models(output.path(), "apple-measured"),
        vec![summary::APPLE_INTELLIGENCE_MODEL_ID]
    );
    assert_eq!(
        saved_models(output.path(), "ollama-only"),
        vec!["qwen2.5:7b"]
    );
}

#[test]
fn private_output_rejects_sibling_worktrees_and_symlinks() {
    let directory = tempfile::tempdir().unwrap();
    let repository = directory.path().join("unrelated");
    std::fs::create_dir(&repository).unwrap();
    let git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .current_dir(&repository)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    };
    git(&["init", "-q"]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--allow-empty",
        "-m",
        "Fixture",
    ]);
    let sibling = directory.path().join("sibling");
    git(&["worktree", "add", "--detach", sibling.to_str().unwrap()]);
    assert!(std::panic::catch_unwind(|| outside_repo(&sibling)).is_err());
    assert!(std::panic::catch_unwind(|| outside_repo(&repository)).is_err());
    #[cfg(unix)]
    {
        let linked = directory.path().join("linked");
        std::os::unix::fs::symlink(&sibling, &linked).unwrap();
        assert!(std::panic::catch_unwind(|| outside_repo(&linked)).is_err());
    }
    assert_eq!(
        outside_repo(directory.path()),
        directory.path().canonicalize().unwrap()
    );
}

#[test]
fn prepare_reads_selected_meeting_without_migrations_or_database_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("private.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE meetings(id TEXT,title TEXT,duration_seconds REAL,edited_transcript TEXT,notes TEXT,participants TEXT);
        CREATE TABLE segments(meeting_id TEXT,text TEXT,start_time REAL,end_time REAL,is_final INTEGER,language TEXT,confidence REAL,speaker TEXT,sort_order INTEGER);
        INSERT INTO meetings VALUES('synthetic','Synthetic',1800,NULL,'Independent notes','[]');
        INSERT INTO segments VALUES('synthetic','Known fact',0,2,1,'fr',NULL,'me',0);").unwrap();
    drop(db);
    let before = std::fs::read(&path).unwrap();
    let inputs = db_inputs(&path, &["synthetic".into()]);
    assert_eq!(inputs[0].segments[0].text, "Known fact");
    assert_eq!(inputs[0].notes.as_deref(), Some("Independent notes"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
#[should_panic(expected = "outside every Git checkout")]
fn private_output_rejects_repository_directory() {
    outside_repo(Path::new(env!("CARGO_MANIFEST_DIR")));
}
