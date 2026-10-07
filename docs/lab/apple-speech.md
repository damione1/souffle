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

Loading creates a SpeechAnalyzer session on the engine actor and negotiates its
actual audio format/sample rate. Rust supplies mono float32 at that rate; Swift
converts the PCM representation to the exact analyzer format (including Int16),
without resampling or changing frame counts. Rust copies PCM at the FFI call boundary; async
Swift tasks never borrow Rust buffers. A bounded input queue reports overflow as
an error. Apple's `timeIndexedProgressiveTranscription` preset emits volatile
revisions with audio ranges, followed by immutable finals. Its `fastResults`
option uses a shorter context window, trading some recognition accuracy for
responsiveness. Rust maps each revision to the shared lane preview contract;
empty previews retract, finals replace the preview, and only finals persist.
Ranges are bounded by consumed PCM, never callback time. Stop finishes the input
and finalizes through its end before draining. Session resets cancel the old
analyzers. A timeline-preserving reset carries the consumed-audio offset.

Apple uses the typed `SilenceHandling::Continuous` policy retained from the
published review branch. VAD still observes speech, but mono input feeds every
sample, including long pauses: a delayed Speech result keeps its original source
range rather than receiving a gap offset computed at delivery. Independent
meeting lanes also feed their complete aligned PCM, including silence padding.
Other engines retain `Gate` and bounded lookback. Continuous input continues
Speech processing during silence; no CPU or power saving is claimed.

Recovery commits each lane's accepted interval before fallible cancellation or
restart. Both new lanes resume at the latest accepted capture time, including
when only one previous push succeeded. A failure starting either source, or a
negotiated-rate mismatch, cancels the partial replacement and leaves that offset
available for retry without counting the old samples twice. A new recording
explicitly resets the offset to zero.

On 2026-10-04, the user expanded the original SOU-121 scope to include the
progressive stream and Me/Them behavior added for Whisper/Parakeet. Meetings now
use independent analyzers for microphone (Me) and system audio (Them), through
the shared `supports_diarization` / `transcribe_dual` interface. These labels
identify capture sources, not inferred human speakers. Asymmetric frames receive
silence padding to maintain a common capture clock; one lane's final never clears
the other's preview. The existing shared live/history paragraph policy is used
without Apple-specific grouping or UI branches. Dictation remains a single lane
without speaker labels. If one analyzer fails while draining, the other lane's
completed finals remain available to the actor's existing salvage hook.

System assets are shared by macOS. The UI shows the equivalent supported locale
(localized language/region name plus stable code) and offers no app-file deletion
control; the backend also refuses deletion. No system assets are removed by
unload or session cancellation. Settings hides the file-model unload timer for
Apple Speech without overwriting its persisted value. The existing idle timeout
still cancels analyzers and drops the engine; macOS owns the model assets and
their memory lifecycle. Apple Speech is not permanently loaded by Soufflé.

Language is selected from `Locale.current`, then negotiated through
`SpeechTranscriber.supportedLocale(equivalentTo:)`. It is independent of Siri's
language and Apple Intelligence. Change it in **System Settings > General >
Language & Region**, then reopen Soufflé: the support catalogue intentionally
keeps its startup snapshot throughout a process, including active recordings.
The UI language changes only the display names, supplied by Foundation for FR/EN.

Unsupported Apple Speech remains discoverable below the model selector, with a
typed translated reason. Language/resource failures link to Language & Region;
an old OS links to Software Update. Unsupported hardware/builds and failed
checks explain the relevant alternative without suggesting a Siri toggle. The
Settings destinations use the installed macOS extension identifiers and an
exhaustive Rust/Slint enum.

Apple references: [SpeechAnalyzer WWDC25](https://developer.apple.com/videos/play/wwdc2025/277/)
explains explicit transcriber locales, system-owned assets, and the independence
from Siri/dictation toggles; [Language & Region settings](https://support.apple.com/guide/mac-help/change-language-region-settings-on-mac-intl163/mac)
describes the macOS language/region controls.

## Observed evidence and pending QA

The observations below belong to the original advanced local history, not a
fresh test of the reconciled build. On 2026-10-07 the user requested combining
advanced local commit `53018512de00cd324421859290e63b51417e8016` with published
review head `25a3085b83bfc4bd9da730b75785b0d02e033ac5`. The original ticket's
mixed-only/no-Me-Them limitation is superseded by the explicitly expanded user
scope above. New build and human acceptance results must be recorded separately
after validation; CodeQL is explicitly delegated to the user.

2026-09-30, Mac16,7 / Apple M4 Pro / 48 GiB / macOS 27.0 (26A428), CLT Swift 6.3.3:
real Speech SDK probe returned hardware available, current locale
`en_US@rg=cazzzz`, equivalent supported locale `en_US`, asset status `supported`
(not installed). The real Swift bridge compiled against the installed SDK.

At that initial September check, no asset installation, live microphone
transcription, PTT/toggle+polish or mixed meeting was exercised. Those were pending
human acceptance checks. A status probe proves API reachability, not ASR output.
The PR was kept draft for review of the observation and stub/runtime lifecycle
paths. Use an isolated profile, explicitly select Apple Speech to install,
transcribe a known phrase, stop/restart, and reselect Kyutai. Never purge shared
Speech assets as part of a test.

2026-10-04: live loading exposed an invalid float32-only format assumption. The
installed en_US assets require interleaved mono Int16 at 16 kHz. The bridge now
converts Rust float32 PCM to that negotiated format. The opt-in integration test
`real_bridge_transcribes_pcm_and_restarts` transcribed the synthesized phrase
"Today we are testing speech recognition on this computer." exactly, checked
final timestamps/no speaker labels, then exercised flush, restart and unload.
This proves the real PCM bridge, not microphone/PTT/toggle/meeting UI acceptance.

2026-10-04, progressive/source parity: the real integration test also receives
nonempty previews **before Stop** in mono and both source lanes. It recognizes the
same phrase exactly in each lane, preserves timestamps across asymmetric input,
drains twice safely, and checks timeline-preserving recovery followed by a new
session reset. `volatileResults` alone did not emit a preview before the short
fixture ended; the official progressive preset passed this check. Deterministic
tests cover revised previews, empty withdrawals, independent lanes, bounded
timestamps, asymmetric PCM, preview suppression during catch-up and final salvage.

To repeat (explicitly reserves/installs system assets for the supported locale):

```sh
say -v Samantha -o /private/tmp/sou-121-known-phrase.aiff 'Today we are testing speech recognition on this computer.'
afconvert -f WAVE -d LEF32@16000 -c 1 /private/tmp/sou-121-known-phrase.aiff /private/tmp/sou-121-known-phrase.wav
SOUFFLE_SPEECH_TEST_WAV=/private/tmp/sou-121-known-phrase.wav \
SOUFFLE_SPEECH_TEST_TEXT='Today we are testing speech recognition' \
cargo test --manifest-path app/Cargo.toml -p souffle --lib \
  engine::apple_speech::tests::real_bridge_transcribes_pcm_and_restarts -- --ignored --nocapture
```

The fixture must use mono float32 at the negotiated sample rate, and its phrase
must match the system Speech locale; this example is for en_US at 16 kHz.

Published review head `25a3085b`, 2026-10-05, separately negotiated mono 16000 Hz
interleaved Int16 and recognized “The quick brown fox jumps over the lazy dog.”
on three runs: 0–2.76 seconds initially, 3.67075–6.43075 after a preserving reset,
and 0–2.76 after a new session. The Rust test observed installed en_US assets;
its subsequent native catalogue example reported supported but not installed.
No installation/removal occurred between those probes and the discrepancy
remains unexplained. The old forced-stub suite passed 119 tests with 8 ignored.
These are review-branch observations, not new evidence for the reconciliation.
Its installed-assets-only `real_bridge_known_phrase_and_session_resets` test is
retained alongside the advanced progressive/source probe; it now selects finals
for phrase assertions instead of mistaking volatile revisions for persisted text.

The combined source is local commit `92bd2a1b`, followed by merge `d30612fe` with
the newest accessible develop `0004eced`. SOU-166's dictionary/paragraph-editor
contract and behavior remain; the old edit-learning bool is replaced by its
typed learning-mode state. New Nightly and human acceptance results will be
recorded separately in [the QA dossier](../qa/sou-121/README.md). No historical
image, native result or test count is relabeled as fresh reconciliation proof.

2026-10-05, Settings follow-up: the user confirmed the preceding transcription
build worked and accepted the rebuilt Settings screen. The local build completed
with `make nightly`, reopened the Nightly bundle, and was signed with Developer ID
Application / team `X6H966RSDB` (bundle `com.souffle.desktop.nightly`). A subsequent
`codesign --verify --deep --strict` passed. The app was left running for testing;
no system assets, app data or TCC permissions were purged.

Validation for this Settings change:

- Workspace tests: 1,464 passed, 19 ignored (including opt-in native ASR tests).
- Workspace Clippy, all targets, with warnings denied: passed.
- Rust formatting, diff whitespace and the centralized FR/EN translation gate:
  passed.
- Deterministic tests cover effective-locale/name preservation, all six typed
  unavailable reasons, exclusion of unavailable models from selection, reactive
  FR/EN display-name switching, and preservation of the stored unload timeout.
- The `apple_speech_settings` Rust example provides an isolated native rendering
  harness for unavailable cases without changing macOS language, Siri, the real
  model selection or user data. It was checked by Clippy; a separate capture
  campaign for these synthetic cases was not run for this follow-up.

Local validation logs: `/tmp/souffle-pr364-settings-tests.log`,
`/tmp/souffle-pr364-settings-clippy.log` and
`/tmp/souffle-pr364-settings-nightly.log`. The signed debug build reports a linker
compact-unwind size warning; the build and signature verification succeed.

## Primary references

- [Apple SpeechAnalyzer](https://developer.apple.com/documentation/speech/speechanalyzer)
- [Apple AssetInventory](https://developer.apple.com/documentation/speech/assetinventory)
- [Apple progressive transcription preset](https://developer.apple.com/documentation/speech/speechtranscriber/preset/timeindexedprogressivetranscription)
- [Apple fast-results latency/accuracy tradeoff](https://developer.apple.com/documentation/speech/speechtranscriber/reportingoption/fastresults)
- [WWDC25 session 277](https://developer.apple.com/videos/play/wwdc2025/277/)

The installed SDK interface was also checked for locale equivalence, actual asset
status, bestAvailableAudioFormat, AnalyzerInput bufferStartTime and finalization.
