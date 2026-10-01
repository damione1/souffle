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
float32 mono format/sample rate. Rust copies PCM at the FFI call boundary; async
Swift tasks never borrow Rust buffers. A bounded input queue reports overflow as
an error. Final-only results use Speech result ranges, and stop finishes the input
and finalizes through its end before draining. Session resets cancel the old
analyzer. A timeline-preserving reset carries the consumed-audio offset. Apple
Speech does not support Me/Them: system-audio meetings use the existing mixed path.

System assets are shared by macOS. The UI names them explicitly and offers no
app-file deletion control; the backend also refuses deletion. No system assets
are removed by unload or session cancellation.

## Observed evidence and pending QA

2026-09-30, Mac16,7 / Apple M4 Pro / 48 GiB / macOS 27.0 (26A428), CLT Swift 6.3.3:
real Speech SDK probe returned hardware available, current locale
`en_US@rg=cazzzz`, equivalent supported locale `en_US`, asset status `supported`
(not installed). The real Swift bridge compiled against the installed SDK.

No asset installation, live microphone transcription, PTT/toggle+polish or mixed
meeting was exercised during this expedited batch. These are explicit pending
human acceptance checks. A status probe proves API reachability, not ASR output.
The PR remains draft until this observation and the stub/runtime lifecycle paths
are reviewed. Use an isolated profile, explicitly select Apple Speech to install,
transcribe a known phrase, stop/restart, and reselect Kyutai. Never purge shared
Speech assets as part of a test.

## Primary references

- [Apple SpeechAnalyzer](https://developer.apple.com/documentation/speech/speechanalyzer)
- [Apple AssetInventory](https://developer.apple.com/documentation/speech/assetinventory)
- [WWDC25 session 277](https://developer.apple.com/videos/play/wwdc2025/277/)

The installed SDK interface was also checked for locale equivalence, actual asset
status, bestAvailableAudioFormat, AnalyzerInput bufferStartTime and finalization.
