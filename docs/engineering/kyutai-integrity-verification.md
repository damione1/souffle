# Kyutai integrity verification — SOU-001 / SOU-271

Base: `8e54d47d` (`origin/develop`). Local fixes: `e17df52d` and
`0ae6746f`. No Moshi/Candle fork, weights change, replay, prime, language gate
or proactive refresh-policy change.

## Batch epochs (SOU-001)

The characterization executes the actual `consume_asr_msgs` loop with a
private model seam, replacing only tokenizer decoding and KV state reset.
The LanguageTracker, epoch-credit, clamp and pending/orphan primitives are
the production implementations. Before the fix, a Word followed by EndWord
mapped the end to **166.40 s instead of 107.36 s**. The same test passes after
reset requests are coalesced per lane and applied after all messages are mapped.

The matrix covers Word alone, Word+EndWord, prefixes 0 / 0.48 s, two lanes,
isolation of the other lane, multiple requests, and reset failure. It preserves
the existing error behavior: epoch credit precedes a failing KV reset. Pending
and orphan finals retain their existing ordering. The real Metal Auto-language
ECorp control compares every emitted text and final flag against the original
E1 baseline, including flush: **622 segments / 311 finals**, identical text and
final flags, **zero clamps / future timestamps**, over 3,743,820 accepted samples.
Its result is in the adjacent verification artifacts.

## Atomic capture ingress (SOU-271)

The producer now sends one `DiarizedPair` domain payload. The actor invokes
one `ingest_pair` transition and evaluates `frame_ready` afterwards. Genuine
single-lane ingress retains the existing six-frame padding tolerance.

The FIFO-before-actor characterization failed at seven frames (**14 calls
instead of seven**). The fixed matrix covers 1 / 6 / 7 / 10 frames and a
137-sample tail. Full vectors must reach MockEngine exactly once, in order,
with matching lanes. A real MeetingMixer accumulated ten frames plus 137 samples
before its tick; its actual output reaches MockEngine sample-exactly. Additional
tests cover stop while reset runs, EOS, stale pairs and two successive sessions.

Admission remains **3,000 lane chunks**, not 3,000 pairs: mono costs one,
paired ingress two. Thus 1,500 diarized 20-ms ticks retain the historical
30-second / 1,440,000-float (~5.76 MB PCM) meeting bound, while mono retains
3,000 slots. Atomic reservations also cover mixed backlogs. Payloads own RAII
permits; rejected sends and dropped/consumed payloads release them. Payload clones
share their reservation until the last clone is dropped. EOS is not audio and
still occupies its physical channel slot as before.

The receiver explicitly closes the budget when the actor exits, waking a stop
tail blocked on weighted capacity. A deterministic test signals actual entry into
the condition-variable wait before dropping the receiver; the waiter then returns
failure within one second. No timing sleep is used. Normal stop still sends its
accepted tail before EOS. The callback uses atomic admission; permit release takes
only the short condition-variable synchronization lock, never I/O or engine work.

## Complete SoftOnly controls

Both runs use the **production SoftOnly policy**, explicit Fr prior, temperature
zero, Nightly 1B Metal/BF16, context 750, the complete E4 fixture
`sou-270-continuous-fr-v1` (465.470667 s), and real `transcribe` / `flush`.
There is no policy override and no source silence/replay added. Only one Metal
job ran at a time. The ignored Rust harness observes the pre-forward selected
text token (one frame delayed), raw/mapped timestamps, Words/EndWords, source RMS,
state/refresh progression, complete segments, and EOS/flush.

| Measurement | Run 1 | Run 2 |
|---|---:|---:|
| Accepted samples / source calls | 11,171,296 / 5,819 | identical |
| Lost source samples | 0 | 0 |
| Words / finals | 1,017 / 1,017 | identical |
| Source Words / EndWords / callbacks | 1,016 / 1,014 / 5,819 | identical |
| Flush Words / EndWords / callbacks | 1 / 1 / 19 | identical |
| Refreshes | 3 | 3 |
| Clamps / timestamps ahead of source cursor | 0 / 0 | 0 / 0 |
| Last lexical source progress | 465.36 s | identical |
| First 15-s planned-speech famine | none | none |
| Largest planned lexical gap | 8.57 s | identical |
| Longest non-delimiter accumulation / same-token repeat | 84 / 49 | identical |
| Matched reference words by LCS | 1,109 / 1,232 | identical |
| Unmatched reference words | 123 | 123 |
| Wall duration | 249.552 s | 251.402 s |

All event records and all segment records are byte-identical between the two
runs. The summaries are identical after removing wall duration. Hashes and
15-second throughput bins are retained in `artifacts/sou-001-271/`.

The result is **not perfect transcription health**. Between source 193.68 and
202.40 s, useful lexical progress stops for 8.72 s of cursor time / 8.57 s of
planned speech. Non-delimiter accumulation lasts 84 callbacks (194.56–201.20 s),
including 49 repeats of the same lexical token (197.36–201.20 s, delayed callback
coordinates). Production SoftPause resets at 201.20 s and 293.84 s; the third
refresh happens during flush. The tail of clause eight is partly omitted around
the first reset. No five-second gap or arbitrary timer is introduced as a policy.

Six exact sentinel spellings are absent (`azur`, `ambre`, `nacre`, `grenat`,
`saphir`, `cristal`). This does **not** mean six clauses are lost: examples include
`azur` → `azure` and `nacre` joined into `balisénacre`. LCS is content screening,
not a WER score or phoneme alignment; the 123 unmatched reference words mix
recognition differences and actual omission. Source RMS activity totals 421.52 s;
planned speech intervals include natural pauses and total 462.920667 s.

## SOU-272 decision and limits

**No SOU-272 implementation.** Neither complete deterministic SoftOnly control
reproduces SOU-270's defined famine of at least 15 seconds of planned source
speech without useful Word progress. Automatic production refreshes resume
progress and it continues until EOS. The transient degeneration and partial
omission remain explicitly recorded above; this result does not establish perfect
ASR quality, validate a reactive detector, or promise absence of famine on other
audio/models. SOU-272 still needs detector calibration and dual-lane recovery
validation if that mitigation is pursued. No token-specific reset, fixed-time
reset, replay/prime/gate, or second audio buffer was added.

## Acceptance evidence

- SOU-001: `batch_epoch_reset_preserves_all_message_times` is the red/green seam
  characterization; `batch_epoch_matrix_covers_pending_lanes_and_reset_failures`
  covers the required lane/message/error matrix. Existing pending/orphan and
  tail tests remain green. `language_mismatch_fixture_preserves_text_and_finals`
  verifies the actual Auto-language model output and flush against E1.
- SOU-271 AC1/AC2: `diarized_bursts_queued_before_actor_stay_aligned`, plus the
  exhaustive `AudioMessage::DiarizedPair` / `SessionMode::ingest_pair` path.
- SOU-271 AC3: unchanged six-frame tolerance, exercised by
  `diarized_mode_waits_out_normal_lane_jitter` and the silent-lane padding tests.
- SOU-271 AC4: `the_queue_holds_the_whole_startup_cap_of_a_diarized_meeting`,
  `mixed_queue_preserves_lane_capacity_and_releases_on_drop` and
  `past_the_cap_chunks_are_dropped_and_counted_and_the_queue_keeps_its_order`.
- SOU-271 AC5: `atomic_pairs_survive_stop_during_reset_and_session_changes`,
  the 137-sample tail matrix, and
  `closing_receiver_wakes_a_tail_waiting_for_weighted_capacity`.
- SOU-271 AC6: `meeting_mixer_burst_reaches_actor_with_both_lanes_exact`
  uses the real mixer with more than 560 ms accumulated before its tick.
- Post-fix SOU-270 controls: two complete real SoftOnly runs with identical
  event/segment streams; SOU-272 is deliberately not implemented for the reason
  above. Live UI/capture QA and detector calibration are not claimed.

## Reproduce

The required local gate completed in order: sidecar test/build, fmt check,
workspace Clippy with warnings denied, workspace tests (**1,279 passed, zero
failed, 13 ignored**), Slint translations, and CodeQL. Every command exited zero.
CodeQL emitted zero SARIF results across Rust, JavaScript and Actions, but this
is not a claim of complete static-analysis coverage: extraction diagnostics
include generated build outputs and an existing `frontmost.rs` panic macro.
Exact commands, counts, limits and SARIF hashes are retained in
`artifacts/sou-001-271/gate.json`.

Run unit characterizations with:

```sh
cargo test --manifest-path app/Cargo.toml -p souffle --lib batch_epoch
cargo test --manifest-path app/Cargo.toml -p souffle --lib diarized_bursts
cargo test --manifest-path app/Cargo.toml -p souffle --lib meeting_mixer_burst
cargo test --manifest-path app/Cargo.toml -p souffle --lib atomic_pairs_survive
cargo test --manifest-path app/Cargo.toml -p souffle --lib backlog_cap_tests
```

For each long control, set `SOUFFLE_VERIFY_FIXTURE` to the E4 WAV,
`SOUFFLE_VERIFY_MODEL` to the complete local Nightly Candle model directory, and
`SOUFFLE_VERIFY_DIR` to a fresh output directory; then run:

```sh
cargo test --manifest-path app/Cargo.toml -p souffle --lib soft_only_full_fixture -- --ignored --nocapture
```

For the Auto-language ECorp control, use the ECorp incident fixture and set
`SOUFFLE_VERIFY_BASELINE` to the E1 baseline `segments.jsonl`; select
`language_mismatch_fixture_preserves_text_and_finals` instead. Model/fixture
hashes and toolchain versions are recorded in the manifest. The large WAV and
weights are referenced in place and not copied into this branch.
