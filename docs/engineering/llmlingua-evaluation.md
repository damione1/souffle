# LLMLingua-2 private evaluation (SOU-106)

This extends the SOU-102 benchmark only. It does not enable compression in the
app. The approved corpus and gold remain unchanged, outside every Git checkout.
The official Python package runs separately outside this repository; neither
Python nor a classifier becomes an application dependency.

## Export production turns

Use the same approved three-meeting French corpus, with 15–25 facts per meeting,
as the baseline. Set the absolute private paths below. A new output directory is
required for each export or run; existing artifacts are never overwritten.

```sh
export SOUFFLE_BENCH_CORPUS_DIR=/absolute/private/corpus
export SOUFFLE_BENCH_OUTPUT_DIR=/absolute/private/production-turns
SOUFFLE_BENCH_MODE=export-turns \
  cargo run --locked --manifest-path app/Cargo.toml --bin souffle-summary-bench
```

The `<meeting-id>.source.json` files contain the exact production turn grouping,
timestamps, speaker labels, calendar participant names, original corpus/gold
hashes and SHA-256 of the turns joined with one newline. Export performs no
provider requests. Edited transcripts are rejected for compression experiments:
replacing them with segment-derived turns would change the baseline input.

## Freeze candidate inputs outside the repository

For each checkpoint, requested rate and device, use a separate private variants
directory. Run the official `PromptCompressor(..., use_llmlingua2=True)` against
the exported turns. Disable context filtering and use `compressed_prompt_list`
so each original turn has exactly one compressed turn. Preserve labels exactly;
the harness rejects removed/reordered turns, changed labels and inserted line
boundaries. Use the ticket's forced tokens/digits and chunk-end tokens. Do not
silently repair classifier output before claiming it passed these checks.

The reference experiment uses the recorded **`protected-turn-prefix-v1`**
policy: split each exported turn's exact timestamp/speaker prefix, including
its following space; classify only the body; then reattach the original prefix
verbatim. Each body still receives participant/digit protections and is kept
exactly as returned by the official package, without restoring deleted words
or negations. The initial full-turn pilot normalized all 16 label spacings;
retain that rejected pilot separately. Record the policy, runner hash, model
revision/options and source/candidate hashes with the private artifacts. Compute
diagnostic token/word ratios over the reconstructed complete input, including
unchanged prefixes. The harness's exact-label checks remain unchanged.

Write `<meeting-id>.variant.json` with this shape (synthetic example):

```json
{
  "meeting_id": "synthetic",
  "corpus_sha256": "copy from source export",
  "gold_sha256": "copy from source export",
  "source_text_sha256": "copy from source export",
  "rate": 0.5,
  "method": {
    "kind": "llmlingua2",
    "checkpoint": "microsoft/llmlingua-2-bert-base-multilingual-cased-meetingbank",
    "revision": "0123456789012345678901234567890123456789",
    "package_version": "record the actual installed version",
    "device": "mps",
    "force_tokens": ["[", "]", ":", "Me", "Them", "\n"],
    "force_reserve_digit": true,
    "chunk_end_tokens": [".", "\n"],
    "classifier_elapsed_ms": 123,
    "peak_rss_bytes": 123456789
  },
  "turns": ["[0:00] Me: exemple compact"]
}
```

Add actual participant names to `force_tokens`; the example omits private data.
Pin the checkpoint's actual 40-character commit hash, and archive the Python
environment, model-file hashes, dtype, batch size and runner alongside the
private results. Measure synchronized classifier wall time separately from
model loading and summary generation. `peak_rss_bytes` is process peak resident
memory, not model weights or Metal allocation size; record separate Metal memory
when available. CPU and MPS measurements use separate processes. For the random
word-deletion control, use `"method": {"kind":"random_words","seed":42}`
with the same rate and label/digit/participant protections, recording its exact
sampling implementation in the private runner.

## Use the unchanged summary pipeline and scoring

```sh
export SOUFFLE_BENCH_VARIANTS_DIR=/absolute/private/mbert-rate05-mps
export SOUFFLE_BENCH_OUTPUT_DIR=/absolute/private/candidate-run
SOUFFLE_BENCH_MODE=run \
  cargo run --locked --manifest-path app/Cargo.toml --bin souffle-summary-bench
```

For a bounded initial run, add
`SOUFFLE_BENCH_CASE=meeting-id,qwen2.5:7b,1`. Case selection still validates the
full approved corpus; it does not constitute the required three-run campaign.
Unload the classifier before loading a summarizer. Serialize campaigns and
respect the shared build/model resource slot.

Each candidate goes through the real `summarize_stream` and structured extraction
with the original notes, participants, language, template and production
parameters. Its immutable artifact is copied into the run directory; each run
records its SHA-256, method, rate and source hash. Baseline runs omit
`SOUFFLE_BENCH_VARIANTS_DIR` and retain their existing behavior.

To rescore, unset `SOUFFLE_BENCH_VARIANTS_DIR` and run with
`SOUFFLE_BENCH_MODE=score` and the saved output directory. Scoring validates the
archived candidate's exact bytes and the original corpus/gold hashes; it does
not read a replacement candidate from today's external variants directory.
The Markdown report identifies frozen variants. Review overrides remain tied
to each exact run's hash, and unreviewed inventions remain unknown.

## Decision stays empirical

Measure both ticket checkpoints at rates 0.5 and 0.7 and their random controls.
Use the same providers and three repetitions as SOU-102. Actual Ollama
`prompt_tokens` diagnostics come from `prompt_eval_count`; package-reported
tiktoken counts and word ratios are diagnostics, not the ticket's actual token
reduction proof. Missing Apple counters remain unavailable.

The existing go criteria at rate 0.5 stay unchanged on all three meetings:
at least 35% actual token reduction; gold recall at least baseline minus one
fact per meeting; no detected meaning inversion after review; classifier time
under 10 seconds for the 75-minute meeting on M-series. Preserve recall by
third, owners and two reviewed before/after extracts in the private report.
A missing/invalid baseline or pending review means **decision pending**, never
a measured go or no-go. SOU-107/108 remain gated on the actual result.

### Input application mode

Variant runs record `input_application_mode: replace_input`: they substitute the input before the production stuff/map decision. Archived runs without this field have the same historical behavior. These experiments do not measure the shipped compaction path, which selects stuff/map from the original transcript and substitutes only map input. A future shipped-path campaign must use a separate, explicit mode; archived variants and scores remain unchanged.
