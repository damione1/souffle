# Experimental meeting summaries and compaction

Meeting summarization and compaction are experimental, **off by default**, and
enabled with **Settings → AI → Summarization** (French: **Résumés de réunion**).
`AppSettings.meeting_summaries_enabled` persists the opt-in. It gates the summary
provider/model/template settings, compaction settings, and meeting generation
UI. The shared AI provider remains configurable for independently enabled
dictation polish. Disabling the feature preserves its subordinate preferences,
saved summaries and compacts, and cancels a pending compression-model download.
Already submitted generation may finish. Startup restores the flag without
starting downloads. Native generation callbacks also reject disabled actions.

Transcription, exports, and read-only MCP access do not require this switch.
Explicit headless summary/compact commands remain available as experimental
invocations without changing the persisted UI preference.

## Why this is experimental

Reliable local transcription is already demanding; faithful summaries of long
transcripts require preserving relationships, decisions and negations as well.
Soufflé targets supported Apple Silicon Macs, including modest configurations.
Machines with 48 GB or more can run larger models and may do better, but that
must not become a requirement for obtaining a useful transcript.

A roughly 15-minute meeting is a more approachable test case, not a fixed cutoff
or a quality guarantee. Apple's on-device Foundation Model has a
[4,096-token session context](https://developer.apple.com/documentation/foundationmodels/managing-the-context-window),
including instructions, input and output. Context windows vary across 7–8B
models, and a large advertised window does not remove practical memory,
latency or long-text faithfulness constraints. Intermediate summary stages and
word extraction are experiments to work within those limits, not semantic
quality guarantees.

For users comfortable with cloud processing, a compatible MCP client can read
original transcripts and ask Claude, ChatGPT or another cloud LLM to summarize
them or maintain an external second brain. Soufflé ships a local, read-only
stdio server, not a direct cloud AI connector or automatic synchronization.
Connection support depends on the client. The client chooses what text leaves
the Mac and where its output is written; Soufflé's MCP server does not write
those notes. This workflow does not require enabling local summarization.

## Local generation options

Once Summarization is enabled, Settings → AI provides two independent,
default-off options: compress long
summary map input, and create a compact transcript alongside a summary. A
standalone Generate/Regenerate action in meeting detail works without any LLM
provider. Short summary input always uses the original text. Missing/downloading
weights cause summaries to proceed immediately using original input; they never
wait for a model download. A successful compact is saved before summary calls,
even if the later summary fails.

The compact is **lossy extraction**, not a summary or a verbatim citation source.
The default target retains **65%** of the text. Settings show “A low value omits
words” only below that default; the compact preview uses the saved compact's
target rate for the same warning, independently of the current settings.
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

`mbert-offset-spans-v2` uses the real pinned encoder and learned 768→2 token
classifier in Candle, fp16 on Metal. Dynamic batches contain up to eight windows
of at most 512 pieces including CLS/SEP. The local embedding wrapper avoids
Candle 0.9.2's fp16 negative-infinity attention-mask bug by using finite -10000
padding masks. All keep scores must be finite. Original byte spans preserve
Unicode/spelling and WordPiece groups; words are ranked deterministically,
protected spans are retained, and retained words stay in source order. Turns are grouped within each recording session, in stored session order; protected session markers distinguish local timestamps after a resume. Legacy v1 compacts remain readable but are marked stale.

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

Settings → AI exposes explicit compression-model removal even with Summarization disabled. Removal preserves preferences and saved compacts; it refuses active installation or inference rather than queuing behind them. Shared inference and exclusive installation/removal locks protect the cache across processes. The model is downloaded again only on a later explicit action.

## CLI and MCP

```sh
souffle --compact-meeting MEETING_ID --compression-rate 0.65 --json
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

## Synthetic native fixture

`meeting_compression_fixture` uses a temporary/explicit synthetic database and
the production compression controller without bootstrap, capture or TCC.
`COMPRESSION_FIXTURE_CACHE` selects a verified prepared cache and
`COMPRESSION_FIXTURE_DB` optionally retains the synthetic database between runs.
Its Summarization switch uses the same persisted settings path as the real app;
enable it before testing generation controls.
Rename edits the meeting title by default. For a deliberate source-edit/stale
regeneration test, set `COMPRESSION_FIXTURE_EDIT_SOURCE_ON_RENAME=1`; only this
fixture then routes Rename text to the edited transcript.

Compact preview rows reuse the session-aware paragraph seek action with original
source seconds. Edited, stale, overlapping-session and grouped cross-session
rows have no seek action when their source association cannot be verified.
Expanding the virtualized preview uses its cached row model and does not read
meeting segments on the UI thread.
