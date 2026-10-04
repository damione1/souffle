# SOU-275: common dialogue stream, 2026-10-04

The real engine actor fed the shipped Slint live-view path through
`examples/batch_preview_live.rs`. Both sources start together, become silent,
and resume at 20 seconds in a 30-second fixture. These are real Skia window
captures, cropped and inspected, from the SOU-275 changes on base `c3d00530`.

| Engine | Visible paragraphs after drain | New turns after the pause | Frames | Dropped / skipped |
| --- | ---: | --- | ---: | --- |
| Whisper turbo, Metal | 4 | Them 0:18, Me 0:19 | 300 | 0 / 0 |
| Parakeet TDT 0.6B v3 int8, ORT | 8 | Them 0:20, Me 0:20 | 300 | 0 / 0 |
| Kyutai STT 1B FR/EN, Candle | 5 | Me 0:20, Them 0:21 | 375 | 0 / 0 |

- [Whisper](whisper-resume.png)
- [Parakeet](parakeet-resume.png)
- [Kyutai](kyutai-resume.png)

The initial simultaneous sources legitimately share the first second. Their
later speech opens new timestamped paragraphs instead of filling those first
two slots. The common contract does not make ASR outputs identical: Whisper's
times retain chunk precision; Kyutai's frame precision and its short initial
"Je" paragraph are visible. No word alignment was invented inside phrases.

The UI harness retains the recording screen after stopping its actor so the
final live grouping can be inspected. Its Recording/Checking system audio
labels are synthetic context, not evidence of physical capture permissions.
It used fixture audio, no microphone, TCC changes or user database. This is
correctness QA, not a latency benchmark: the local gate ran concurrently.

Build from the worktree root:

```sh
CARGO_TARGET_DIR=/Users/damien/Projects/souffle/app/target \
  cargo build --manifest-path app/Cargo.toml -p souffle-slint --example batch_preview_live
```

Run from `app/`, changing only the selected engine profile:

```sh
SOU273_ENGINE=kyutai \
SOU273_PCM=/tmp/sou274-mic.json \
SOU274_SYSTEM_PCM=/tmp/sou274-system-overlap.json \
SOU275_HOLD_SECONDS=15 \
/Users/damien/Projects/souffle/app/target/debug/examples/batch_preview_live
```

The two JSON files contain 16 kHz mono samples prepared for SOU-274. The
harness now gets its target rate and hop from `audio_requirements`: 16 kHz /
1,600 samples for the batch profiles, 24 kHz / 1,920 samples for native Kyutai.
It resamples both inputs with the existing production `Resampler`. All three
actors drained and all harness processes exited successfully.

Logs are local at `/tmp/sou275-native/{whisper,parakeet,kyutai}.log`. The
deterministic committed regression tests do not require these temporary files
or installed model weights. They cover alternating word/chunk streams through
live → DB → detail/export, a 15-second delayed lane, preview withdrawal,
sentence limits, jitter and bounded history over hundreds of turns.
