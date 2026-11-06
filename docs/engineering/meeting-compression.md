# Local meeting compression

Settings → IA provides two independent, default-off options: compress long
summary map input, and create a compact transcript alongside a summary. A
standalone Generate/Regenerate action in meeting detail works without any LLM
provider. Short summary input always uses the original text. Missing/downloading
weights cause summaries to proceed immediately using original input; they never
wait for a model download. A successful compact is saved before summary calls,
even if the later summary fails.

The compact is **lossy extraction**, not a summary or a verbatim citation source.
The retained percentage is a target: protected participant names, numbers and
French negations may increase it. Edited transcript text takes precedence over
segments, including plain edited prose. Original timestamp/speaker prefixes and
line boundaries are preserved. Timed compact preview rows seek the same meeting
audio; Copy always copies the full text. Source changes mark prior results stale;
old text stays visible until a successful regeneration.

## Distribution and attribution

Model: Microsoft `llmlingua-2-bert-base-multilingual-cased-meetingbank`, pinned to
revision `5f0c82792b7ea14c6484e015b6a072009496b7f2` on Hugging Face. Every source
file has a pinned byte size and SHA-256 in `models/compression.rs`; all downloads
use the immutable revision. First install downloads approximately **713 MB** of
official fp32 weights and tokenizer/configuration files, then converts weights
locally in Rust to approximately **355 MB fp16**. This is a local derivative,
not a hosted fp16 mirror. No account or Hugging Face token is needed. Upstream
README and local conversion lineage/checksum metadata remain in the cache.

Upstream model license: **Apache-2.0**. Attribution: Microsoft LLMLingua-2,
<https://huggingface.co/microsoft/llmlingua-2-bert-base-multilingual-cased-meetingbank>,
<https://github.com/microsoft/LLMLingua>.
The Apache license text is bundled in `docs/licenses/llmlingua-2-Apache-2.0.txt`.
No upstream endorsement is implied; this app changes the numerical weight dtype
and uses its own documented selection policy.

Cache: `models/compression/llmlingua-2-mbert/<revision>/` beneath the app data
directory. Startup only checks small installation metadata and file sizes.
Full checksum and tensor validation run on a worker before inference. Downloads
are explicit (committed enabling change, Download/Retry, standalone Generate or
headless compact action); persisted toggles never authorize a startup download.
Install staging is private, checksum-verified and atomically published. A process
coordinator plus filesystem advisory lock prevents competing GUI/CLI installers.
Cancellation cleans this job's staging only; a retry can repair an invalid cache.

## Inference and persistence

`mbert-offset-spans-v1` uses the real pinned encoder and learned 768→2 token
classifier in Candle, fp16 on Metal. Dynamic batches contain up to eight windows
of at most 512 pieces including CLS/SEP. The local embedding wrapper avoids
Candle 0.9.2's fp16 negative-infinity attention-mask bug by using finite -10000
padding masks. All keep scores must be finite. Original byte spans preserve
Unicode/spelling and WordPiece groups; words are ranked deterministically,
protected spans are retained, and retained words stay in source order.

This is not byte-for-byte filtering parity with the official Python package:
that package substitutes some force tokens and uses a cl100k-weighted strict
percentile. No Python performance or semantic-quality measurements are claims
about this implementation. There is no app/runtime Python dependency.

One compute permit and blocking load/infer/drop scope release model, tokenizer,
Metal device, tensors and maps before generative summary requests. Recording
start signals cancellation at batch/tensor boundaries; an already submitted
Metal batch may finish. Recording never waits for whole-meeting compression.
Unrelated ASR/Ollama caches are not unloaded.

Schema 18 stores one optional compact object with source fingerprint, model
revision/policy/checksum, target rate, timestamp, stale flag and approximate token
count. Transactional UUID generation and source compare-and-update reject old
jobs after edits, resume, deletion or newer generation. Full saves preserve
existing compact columns. Compact text is not added to FTS a second time.
Deleting a meeting deletes its derived compact as part of the same row.

## CLI and MCP

```sh
souffle --compact-meeting MEETING_ID --compression-rate 0.5 --json
souffle --summarize-meeting MEETING_ID --summary-compression on \
  --summary-provider ollama --summary-model qwen2.5:7b --json
# Use --database /path/to/fixture.db (or SOUFFLE_DB) for an alternate database.
```

CLI overrides apply to this invocation, not saved preferences. `--model` still
selects the transcription model; summaries use `--summary-model`.

MCP `get_meeting` and `get_latest_meeting` accept `include: ["compact"]`.
Omitted/empty include returns all sections. `compact_transcript` is null when
absent and includes `is_stale`, `source_sha256`, provenance and `approx_tokens`
when present. `list_meetings` exposes `has_compact` without reading compact text.
Use summary for an overview, compact for an economical first pass, and transcript
for exact citations. The sidecar remains read-only and can read genuine pre-v18
databases without referencing missing compact columns.
