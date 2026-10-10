# Reproducible private summary benchmark (SOU-102)

The ignored Rust integration test uses the production prose and structured
extraction pipelines. It does not open the application `Database`, migrate SQLite,
change prompts, alter map concurrency or write user settings. Normal application
runs do not collect benchmark diagnostics.

The ignored test builds and launches the Rust `summary_bench` example. Its normal
entry point dispatches the production Apple helper before starting the benchmark.
This preserves process isolation: a libtest executable cannot handle the internal
helper flag. The shared harness lives in `app/tests/support/summary_bench.rs`;
the test wrapper also verifies the helper protocol without calling a provider.

Use absolute directories **outside all repository checkouts**. Outputs contain
private prose, participants, notes and map summaries. Do not commit them.

## Prepare corpus and gold before any quality run

Select at least three French meetings around 30, 45 and 75 minutes. Set:

```sh
export SOUFFLE_BENCH_DB='/absolute/private/souffle.db'
export SOUFFLE_BENCH_MEETING_IDS='id-30,id-45,id-75'
export SOUFFLE_BENCH_CORPUS_DIR='/absolute/private/summary-bench/corpus'
export SOUFFLE_BENCH_OUTPUT_DIR='/absolute/private/summary-bench/baseline'
SOUFFLE_BENCH_MODE=prepare cargo test --manifest-path app/Cargo.toml -p souffle --test summary_bench -- --ignored --nocapture
```

`prepare` uses `SQLITE_OPEN_READ_ONLY`, reads only explicitly selected meetings,
and writes JSON corpus exports and empty gold drafts. Existing corpus/gold files
are never overwritten. Alternatively supply exports using the `Input` shape in
`app/tests/summary_bench.rs`: id, title, duration_seconds, segments,
edited_transcript, notes, participants, language (`fr`), template (builtin id).

Prepare 15–25 independent facts per meeting from transcript/notes **without reading
candidate summaries**. Every fact has id, text, third (`beginning`, `middle`, `end`),
keywords and optional owner. Keyword groups are ANDed; alternatives within one group
are ORed. For example:

```json
{"id":"f01","text":"Alice must publish the migration plan Friday","third":"middle","keywords":[["migration plan","plan de migration"],["Friday","vendredi"]],"owner":"Alice"}
```

Retain decisions, explicitly assigned actions, figures/dates/projects and open
questions across all thirds. Check the corpus SHA256 in each gold. After Damien
explicitly approves/corrects the gold, record `validated_by` and `validated_at`.
The harness refuses quality runs with unvalidated, empty, mismatched or undersized
gold. These fields record a real approval; do not populate them speculatively.

## Run and rescore

Commit the harness first. Use the baseline commit whose production behavior is
being compared. Ensure models `qwen2.5:7b` and `qwen3:4b` are installed in Ollama.
Apple Intelligence is run only when the real bridge is available; otherwise the
report names its reason/stub status. A long Apple run must demonstrate intermediate
merge rounds before later Apple gains can be accepted.

```sh
# Optional: default is http://localhost:11434
export SOUFFLE_BENCH_OLLAMA_URL='http://localhost:11434'
cargo test --manifest-path app/Cargo.toml -p souffle --test summary_bench -- --ignored --nocapture
# After reading every output and editing *.review.json:
SOUFFLE_BENCH_MODE=score cargo test --manifest-path app/Cargo.toml -p souffle --test summary_bench -- --ignored --nocapture
```

Three runs per meeting/configuration are serialized; production map concurrency
remains unchanged. Use a fresh output directory for each baseline/candidate. Each
immutable `*.run.json` records commit, machine/OS, provider metadata, gold/corpus
hashes, template/system prompt, parameters, prose, structured outcomes, map output,
phase calls/tokens/time, map retries/splits and merge rounds. Apple bridge attempts
are counted individually, including retries. Tokens not exposed by a provider are
`null`, never zero. Failed/incomplete calls have an error; provider/extraction
errors make the run invalid rather than assigning zero recall.

`provider-availability.json` freezes the measured machine's Apple availability and
reason. Offline `score` derives each meeting's model set from its saved run files,
so moving artifacts to a machine with different Apple support never creates or
removes measured runs. Older artifacts without availability provenance are labelled
accordingly. Apple hard timeouts are finalized by the waiting thread before a retry;
a late abandoned response cannot turn the timed-out attempt into a success.

`*.review.json` binds to a run hash. Set `matches` fact-id overrides to correct
keyword false positives/negatives, and `reviewed_by` before saving manual decisions.
Set `invented_facts` only after reading the output against the transcript. An
unreviewed output is marked **NOT REVIEWED**, never zero hallucinations. Owners
are counted on matching structured action items, not merely a name appearing
elsewhere in the prose. Inspect semantic attribution and keyword errors manually.
Rejecting a fact through `matches` also rejects its owner attribution.

The command writes per-run scores and `bench-resumes-<date>.md`, including recall
by third, owners, duplicated Summary/Topics headings, calls, available tokens,
wall time, merge rounds and a table of means over valid runs (with valid-run count).
Retain raw outputs, hashes and reviews when comparing candidates. Inspect all
three per-meeting/provider runs rather than interpreting the mean alone.

## Delivery status

Harness and synthetic scoring/diagnostics are deliverable independently of the
private baseline. This PR does not claim a prepared/approved private gold,
three quality runs or measured Apple merge rounds. On 2026-09-30 the stable DB
contained a single 9-second test meeting; Nightly's longest meeting was 256 seconds.
The former representative corpus must be located/supplied before gold preparation
and baseline measurements. SOU-103/104/105 remain gated on those missing proofs.

On 2026-10-05 the user confirmed that corpus was on another machine and requested
skipping this empirical blocker. No corpus was fabricated, no gold approval was
assumed, and no private quality run was performed during PR maintenance.
