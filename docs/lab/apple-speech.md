# Apple Speech engine (SOU-121)

Apple Speech is opt-in in the existing transcription catalogue. Kyutai 1B/Candle
remains the default. SpeechAnalyzer is separate from FoundationModels: polish
and summary providers are unchanged.

A real macOS 26+ Apple Silicon bridge exposes only a locale equivalent to
`Locale.current`. Availability requires actual SpeechTranscriber hardware/locale
support. Builds with older SDKs use a stub; `SOUFFLE_FORCE_SPEECH_STUB=1` exercises
that build path. Unsupported profiles fail resolution with an actionable error,
and the catalogue preserves persisted identifiers so another model can be chosen.

`ModelAssetSource` distinguishes app files from `SystemSpeech { locale }`.
DownloadRequired means supported system assets are not installed. The existing
Download action calls AssetInventory reservation/installation on a worker, and
only the terminal command transition reports Complete. No Hugging Face request
or `models/apple-speech` directory is involved. Failed/timed-out installation
reports an error rather than leaving Downloading. Progress is indeterminate
because the current FFI reports start/completion, not Apple Progress byte counts.

Loading creates a SpeechAnalyzer session on the engine actor and uses its actual
mono sample rate. Rust supplies float32 PCM; an AVAudioConverter converts its
representation to the negotiated system format (including int16), without
resampling and with an exact frame-count check. Rust copies PCM at the FFI call boundary; async
Swift tasks never borrow Rust buffers. A bounded input queue reports overflow as
an error. Final-only results use Speech result ranges, and stop finishes the input
and finalizes through its end before draining. Session resets cancel the old
analyzer. A timeline-preserving reset carries the consumed-audio offset. Apple
Speech does not support Me/Them: system-audio meetings use the existing mixed path.

### Architecture decision requiring maintainer confirmation

Apple uses the typed `SilenceHandling::Continuous` engine policy. The pipeline
still observes VAD, but feeds every frame, including long pauses. A Speech result
can arrive asynchronously after its utterance: removing silence and applying the
current gap offset when that result arrives would move old words into the future.
Continuous input preserves the source clock through delayed delivery and final
flush. Existing engines retain `Gate` and its bounded lookback behavior.

The tradeoff is continued Speech processing during silence; no CPU/power saving
is claimed for paused speech. A more complex source-time gap map is deferred.
Timeline-preserving recovery consumes the previous sample interval before the
fallible restart, so a failed begin followed by retry never counts it twice.
PR #364 remains draft for Damien's review of this architecture choice.

System assets are shared by macOS. The UI names them explicitly and offers no
app-file deletion control; the backend also refuses deletion. No system assets
are removed by unload or session cancellation.

## Observed evidence and pending QA

2026-09-30, Mac16,7 / Apple M4 Pro / 48 GiB / macOS 27.0 (26A428), CLT Swift 6.3.3:
real Speech SDK probe returned hardware available, current locale
`en_US@rg=cazzzz`, equivalent supported locale `en_US`, asset status `supported`
(not installed). The real Swift bridge compiled against the installed SDK.

2026-10-05 review: the Rust test binary reports `en_US` installed. The actual SDK
negotiated mono 16000 Hz Int16 interleaved (`commonFormat=3`), so rejecting any
format other than float32 previously prevented load on this machine. No installation
or removal was performed. The synthetic phrase “The quick brown fox jumps over
the lazy dog.” decoded exactly on three runs at 16000 Hz. Initial final range:
0–2.76 s; after preserving reset: 3.67075–6.43075 s; after new-session reset:
0–2.76 s. All segments were final with no speaker identity. Unload rejected
further transcription. The forced stub engine suite passed 119 tests (8 ignored).
The later isolated native Slint binary instead reported `en_US` supported but
not installed; the cause of this process/time-dependent difference remains
unverified, with no installation/removal requested between probes. Its real
catalogue/FR/EN rendering and this limit are recorded in
[`docs/qa/sou-121`](../qa/sou-121/README.md). Do not infer shipped-app Ready from
the test-binary success.
The deterministic actor regression also verifies delayed pre-pause results after
a pause longer than drain plus lookback, every sample fed, and unchanged stop
timestamps. A failed-then-successful restart regression covers clock accounting.

Reproduce the opt-in real probe without microphone/profile access (assets must
already be installed; the test never installs them):

```sh
say -v Samantha -o /tmp/sou121-phrase.wav --file-format=WAVE --data-format=LEI16@16000 \
  'The quick brown fox jumps over the lazy dog.'
SOUFFLE_SPEECH_TEST_WAV=/tmp/sou121-phrase.wav cargo test --manifest-path app/Cargo.toml \
  -p souffle --lib real_bridge_known_phrase_and_session_resets -- --ignored --nocapture
SOUFFLE_FORCE_SPEECH_STUB=1 cargo test --manifest-path app/Cargo.toml -p souffle --lib engine::
```

Live microphone PTT/toggle+polish, system-mix recording and missing-asset
installation/cancellation remain pending end-to-end acceptance. The real probe
proves ASR/flush/reset but does not stand in for those UI/capture paths. Existing
user Nightly, settings, recordings and shared assets were preserved. Never purge
shared Speech assets to manufacture a missing-assets test.

## Primary references

- [Apple SpeechAnalyzer](https://developer.apple.com/documentation/speech/speechanalyzer)
- [Apple AssetInventory](https://developer.apple.com/documentation/speech/assetinventory)
- [WWDC25 session 277](https://developer.apple.com/videos/play/wwdc2025/277/)

The installed SDK interface was also checked for locale equivalence, actual asset
status, bestAvailableAudioFormat, AnalyzerInput bufferStartTime and finalization.
