---
name: engine-audio-fixtures
description: >
  Use when working on Souffle engine behaviour that depends on audio timing:
  punctuation and sentence breaks, the pending or dropped last word, VAD
  gating, batch windowing cut points, or diarization lane routing. Covers the
  committed spoken-audio fixtures, the tts-fixtures generator that renders
  them, and the ignored integration tests that measure the engine against
  them. Trigger on "pause ladder", "punctuation threshold", "SOU-030",
  "audio fixture", "tts-fixtures", "punctuation_threshold", or any request to
  run, regenerate, extend, or interpret these local engine tests.
---

# Engine audio fixtures

Some engine behaviour can only be measured with speech whose silences have an
exact, chosen length. A recording cannot give that, and nobody can pause for
exactly 300 ms on request. So the speech is synthesized, the gaps between
clauses are inserted as digital silence, and the result is committed as a WAV.

The digital silence is the variable under test. The speech synthesis only has
to produce intelligible clauses.

## Layout

| Path | What it is |
| --- | --- |
| `src-tauri/tts-fixtures/` | The generator. A workspace member, not part of the app. |
| `src-tauri/tts-fixtures/specs/*.toml` | Fixture definitions. Add new ones here. |
| `src-tauri/fixtures/audio/<ticket>/` | The committed WAVs the tests read. |
| `src-tauri/tests/punctuation_threshold.rs` | The SOU-030 measurement. |

## Running the tests

These load the real 2.2 GB Kyutai model, so they are `#[ignore]` and never run
in GitHub CI. `cargo test --workspace` skips them; that is deliberate and the
contracts workflow needs no exclusion.

```bash
cd src-tauri
cargo test -p souffle --test punctuation_threshold -- --ignored --nocapture
```

Prerequisite: the Kyutai STT model must be downloaded. Easiest is to open the
app once and let it fetch `stt-1b-en_fr`. It lands in
`~/Library/Application Support/com.souffle.desktop/models/kyutai/stt-1b-en_fr/candle`.
The test fails with a clear message if it is missing.

Runtime is roughly 30 seconds for the twelve SOU-030 clips on an M-series Mac.

## Reading the output

Each rung prints the pause length, whether the model ended the sentence there,
and the transcript:

```text
=== hesitation-a ===
    100 ms  no break  Je pense que ce serait une bonne idée, il faudrait ...
    300 ms  BREAK     Je pense que ce serait une bonne idée. Il faudrait ...
--- measured first break ---
  hesitation-a: 300 ms
  hesitation-b: 400 ms
```

`BREAK` means a sentence-ending mark appeared somewhere other than the end of
the clip, so the model treated the pause as a full stop.

The two sentences do not break at the same pause length. That is the point:
the break is not a pure function of silence, it also depends on the words. Do
not collapse this to a single threshold number.

### What the test asserts, and what it does not

It asserts only the two anchors that held for both sentences: no break at
100 ms and 200 ms, always a break at 1000 ms, plus a guard that the first
break stays at or below 600 ms. The transition between the anchors is
reported but not asserted, because a fine sweep near the boundary was not
monotonic (260 ms broke, 270 ms and 280 ms did not, 290 ms broke again).

It never asserts which words came back. Synthetic speech is unrepresentatively
easy for an ASR, so a word-accuracy assertion here would measure nothing real.
Assert on plumbing: punctuation placement, timestamps, segment ordering, lane
routing. For anything about recognition quality, use a real recording.

## Regenerating the fixtures

Only needed if a spec changes. The WAVs are committed precisely so the
measurement does not shift when macOS changes its speech synthesis.

```bash
cd src-tauri
cargo run -p souffle-tts-fixtures -- tts-fixtures/specs/sou-030-punctuation.toml
```

Useful flags: `--only <fixture-name>` to render one, `--dry-run` to list what
would be written.

If regenerating changes the committed WAVs, re-run the test and update the
baseline numbers in `punctuation_threshold.rs` and in the SOU-030 ticket. A
silently shifted fixture invalidates the recorded measurement.

## Adding a fixture

Edit a spec, or add a new `.toml` beside it. Two forms:

```toml
sample_rate_hz = 16000
out_dir = "fixtures/audio/sou-030"      # relative to src-tauri

[backend]
kind = "macos_say"
voice = "Thomas"                         # `say -v '?'` lists voices
rate_wpm = 180

# A ladder renders one WAV per gap value, same clauses each time.
# Names come out as "<name>-gap0300ms".
[[ladder]]
name = "hesitation-a"
clauses = ["je pense que ce serait une bonne idée",
           "il faudrait vraiment en reparler demain matin"]
gaps_ms = [100, 200, 300, 400, 600, 1000]

# A plain fixture is an explicit sequence, for cases that are not a ladder.
[[fixture]]
name = "three-clause-example"
clauses = ["première partie", "deuxième partie", "troisième partie"]
gaps_ms = [250, 900]        # one value reuses it between every pair
lead_ms = 300               # optional, defaults shown
trail_ms = 800
```

Each distinct clause is synthesized once and reused across rungs, so a ladder
varies only in silence.

Then add the fixture names to whichever test consumes them. The test reads
files by name, so a spec change and a test change go together.

## Why macOS `say` and not Kyutai TTS

Kyutai TTS was the intended backend, since it is the same model family as the
STT and speaks good French. It does not work from Rust today.

The `moshi` crate (0.6.4) exposes `tts` and `tts_streaming`, but neither can
load `kyutai/tts-1.6b-en_fr`. The checkpoint stores the depformer as one
shared 4-layer transformer plus 11 scheduled input projections
(`depformer_in.0..10`) and top-level `linears.0..31`. The Rust `DepFormer`
expects a fully replicated transformer per slice (`depformer.<slice>.*`). The
checkpoint also sets `demux_second_stream`, which has no Rust support at all.
Kyutai's own shipped `config-tts.toml` for this model uses `type = "Py"`,
confirming the Rust path targets older T5-based checkpoints.

The Python implementation does work, but it needs torch (~2.5 GB) plus the
4.1 GB checkpoint.

`say` is adequate here because the measured variable is the inserted silence,
not the speech. Its limits, worth stating in any result:

- Prosody at a clause boundary is flatter than natural speech. A semantic VAD
  partly keys on intonation, so absolute thresholds may sit slightly differently
  for real voices. Relative comparisons between rungs stay valid.
- English and French voices only, whatever the OS ships.

The generator hides the backend behind a `SpeechBackend` trait, so swapping in
Kyutai TTS later means adding one `impl` and a `kind` value in the spec.

## Related

- Ticket SOU-030, in the Obsidian vault under `Projects/Souffle/Bugs/`.
- `parakeet_real_inference` in `src-tauri/src/engine/parakeet.rs` is the older
  precedent for an ignored, model-loading test. It reads a hand-made file from
  `/tmp`; prefer a committed fixture instead.
