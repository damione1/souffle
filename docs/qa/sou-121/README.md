# Apple Speech reconciliation and historical evidence

The 2026-10-07 reconciliation combines advanced local `53018512` (locale/display
names, typed actionable availability, progressive results, independent Me/Them)
with published `25a3085b` (Continuous silence policy, safe failed restart and
negotiated PCM). Me/Them is now explicitly within the user's scope; the older
ticket's mixed-only constraint is stale. The published Continuous behavior and
actor regression remain, and recovery coverage now includes mono and either
source's failed begin, mismatched rates and delayed finals in both lanes.

All images and traces already below are historical. They are not new proof of
the reconciled source. Fresh compile/test/build and human validation will be
recorded separately; CodeQL is user-owned and is not run for this reconciliation.
The older `apple_speech_catalog` example has been superseded by the
`apple_speech_settings` harness, which projects locale names and typed reasons
without a misleading synthetic available/Ready claim.

## Historical advanced local progressive/source parity — 2026-10-04

Real Apple Speech on macOS 27.0 (26A428), Apple M4 Pro, en_US assets,
debug Skia native replay built on `cc473fd1` plus the source-parity changes.
The user expanded SOU-121's original final-only, mixed-source scope on October 4.

The real engine actor feeds the shipped Slint live-view path through
`examples/batch_preview_live.rs`. Two 35-second, 16 kHz mono source fixtures
contain overlapping speech, a pause and source reentry:

- Microphone: Samantha at 0.2 and 20 seconds: “I am speaking through the
  microphone. We need progressive speech recognition so each new sentence
  appears while the meeting is still running.”
- System: Alex at 4 and 24 seconds: “This is the system audio speaking. Keep
  the two sources separate and show my words in the other speaker column.”

Observed and inspected:

- [Progressive screenshot](apple-progressive.png): four source paragraphs;
  the second Them phrase remains gray and revisable while capture continues.
- [Completed screenshot](apple-final.png): Me 0:00, Them 0:04, Me 0:20,
  Them 0:24, with final replacements and no duplicated preview text.
- [Full actor/UI trace](native-replay.txt): first Me preview at 1.213 seconds,
  successive phrase revisions, true finals, 350 processed frames, zero dropped
  or skipped chunks. This is one correctness run, not a latency benchmark.

The harness retains the recording screen after stopping its actor to inspect
the completed live transcript. Its Recording/Checking system audio labels are
synthetic context. The replay uses fixture audio, no microphone or system tap,
no TCC changes and no user database. It proves actual ASR → actor → shared
paragraph grouping → native Slint rendering, not physical capture acceptance.

Build from the worktree root:

```sh
cargo build --manifest-path app/Cargo.toml -p souffle-slint --example batch_preview_live
```

Run from `app/` (local fixture files contain exactly 560,000 float32 samples
serialized as JSON per lane; generated using macOS `say`, ffmpeg and a temporary
Rust float32-to-JSON converter):

```sh
SOUFFLE_APP_IDENTIFIER=com.souffle.desktop.nightly \
SOU273_ENGINE=apple-speech \
SOUFFLE_REPLAY_INSTALL_ASSETS=1 \
SOU273_PCM=/private/tmp/sou121-stream-qa/mic.json \
SOU274_SYSTEM_PCM=/private/tmp/sou121-stream-qa/system.json \
SOU275_PCM_RATE=16000 SOU275_HOLD_SECONDS=60 \
./target/debug/examples/batch_preview_live
```

The explicit install flag uses the ordinary model download API to reserve the
already installed Speech assets for this separate test process. No shared assets
are deleted. The harness loads `model_location` and uses the negotiated audio
requirements, without branching on engine identifiers.

Capture used `System Events` to read the actual `batch_preview_live` window
bounds (344, 91, 1040, 852), then `screencapture -x -R 344,91,1040,852`.
Both committed images were opened and inspected in this session using the
`souffle-ui-screenshot` skill. The harness exited cleanly after its hold interval.

The separate opt-in real-engine test in `engine/apple_speech.rs` checks exact
known-phrase finals, previews before Stop in mono and both independent source
lanes, asymmetric silence padding, repeated drain, recovery offset and reset.
Repeat its fixture command from [the engine notes](../../lab/apple-speech.md).

Physical microphone/PTT/toggle-plus-polish and a live system-audio meeting remain
separate acceptance checks. The user had confirmed microphone transcription with
the preceding PCM-fixed build; this replay does not replace that capture test.

Local validation passed:

```sh
cargo fmt --manifest-path app/Cargo.toml --all -- --check
cargo clippy --manifest-path app/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path app/Cargo.toml --workspace
./scripts/check_slint_translations.sh
```

Workspace tests: 1,461 passed, 19 opt-in tests ignored, no failures. The real
Apple integration test was run separately with its known-phrase fixture.

`make nightly` subsequently rebuilt source commit `6e0c64d7`, signed it with
Developer ID Application: Damien Goehrig (X6H966RSDB), verified the signature,
and launched `com.souffle.desktop.nightly`. The actual window showed
[Ready Apple Speech](nightly-ready.png). The full window was captured and
inspected locally; this committed header capture records readiness without
including the existing transcription history. Nightly was left running.
