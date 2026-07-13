<div align="center">

<img src="docs/souffle-logo.svg" alt="Soufflé logo" width="120">

<h1>Soufflé</h1>

<p><strong>Private speech-to-text for macOS that never leaves your Mac.</strong></p>

<p>
  Dictate into any app, transcribe meetings with speaker separation, and get on-device summaries<br>
  with decisions and action items. No cloud, no accounts, no API keys.
</p>

<p>
  <img alt="License: GPL v3" src="https://img.shields.io/badge/License-GPLv3-blue.svg">
  <img alt="Tauri 2" src="https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white">
  <img alt="Svelte 5" src="https://img.shields.io/badge/Svelte-5-FF3E00?logo=svelte&logoColor=white">
  <img alt="macOS Apple Silicon" src="https://img.shields.io/badge/platform-macOS%20Apple%20Silicon-lightgrey">
</p>

<p>
  <a href="#download"><strong>Download</strong></a> ·
  <a href="#speech-models">Speech models</a> ·
  <a href="#build-from-source">Build from source</a>
</p>

</div>

<p align="center">
  <img src="docs/screenshots/meeting-live.png" width="820" alt="Live meeting transcription running fully on-device, separating Me from Them in real time">
</p>

Everything runs on-device:

- 🔒 **Fully private.** Transcription, summaries, and audio all stay on your Mac. Nothing is uploaded, and it works offline.
- 🎙️ **Dictation and meetings.** Talk into any app with a global shortcut and auto-paste, or capture a meeting with a live transcript that tells you apart from everyone else.
- 🧠 **Understand and own.** On-device summaries with decisions and action items, full-text search, and export to Markdown, JSON, or subtitles.

## Transcribe

- **Dictation**, with auto-paste into whatever app you were using and a global shortcut to start it from anywhere. Apps that reject synthetic paste (terminals, secure fields) can receive simulated keystrokes instead. Optional start/stop sounds confirm the shortcut landed.
- **Meeting transcription**, with a live transcript and system-audio capture that separates Me from Them. Optional audio recording keeps the meeting sound as compact Opus files with a retention policy, replayable with click-to-seek from the transcript.
- **Hands-off recording lifecycle**: the app offers to start when a calendar meeting begins, detects when the meeting seems over and stops on its own after warning you, survives lid-close and system sleep by pausing and resuming, and recovers or salvages the session if the engine stalls or the microphone disappears.

| Dictate into any app | Your timeline, grouped by day |
| :---: | :---: |
| ![Live dictation view with the transcript as the whole surface and auto-paste on stop](docs/screenshots/dictation.png) | ![Home timeline grouping meetings and dictations by day](docs/screenshots/timeline.png) |

## Understand

- **Meeting summaries**, generated on-device by Ollama or Apple Intelligence (no setup when Apple Intelligence is available).
- **Structured outcomes**: decisions, action items with owners, and open questions extracted alongside the summary.
- **Dictation polish** (optional): a local LLM pass cleans up dictated text with editable prompt templates before pasting.
- **Full-text search** across every transcript and dictation entry.

| Transcript, notes, and participants | On-device summary and outcomes |
| :---: | :---: |
| ![Meeting detail with editable notes and a Me/Them transcript](docs/screenshots/meeting-detail.png) | ![Generated decisions, action items with owners, and open questions](docs/screenshots/summary.png) |

## Own your data

- **Export any meeting** as Markdown, JSON, or SRT/VTT subtitles, or the **whole archive** as a plain folder of Markdown and JSON.
- **MCP server**: the bundled `souffle-mcp` sidecar lets Claude Desktop, Claude Code or any MCP client search and read your transcripts. Read-only, fully local, works even when the app is closed. Setup snippets live in Settings > Data.
- **Headless CLI**: `souffle --transcribe-file audio.wav --json` transcribes a file without launching the app, and `--repeat N` doubles as a benchmark harness.

  The `souffle` binary ships inside the app bundle and is not added to your `PATH`, so it is not a global command. Invoke it by full path, or symlink it once:

  ```bash
  # Run directly
  "/Applications/Soufflé.app/Contents/MacOS/souffle" --list-engines

  # Or expose it as a `souffle` command
  ln -s "/Applications/Soufflé.app/Contents/MacOS/souffle" /usr/local/bin/souffle
  ```

## Speech models

All models run locally and are downloaded on first use from HuggingFace:

- [Kyutai STT 1B](https://huggingface.co/kyutai/stt-1b-en_fr-candle) (default): French + English streaming transcription, ~2.4 GB, Metal GPU via Candle
- [Kyutai STT 2.6B](https://huggingface.co/kyutai/stt-2.6b-en-candle): English, higher quality, ~5.6 GB
- [Whisper Large V3 Turbo](https://huggingface.co/ggerganov/whisper.cpp): multilingual, ~1.6 GB, Metal via whisper.cpp
- [Parakeet TDT 0.6B v3](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx): 25 languages with punctuation and capitalization, ~670 MB int8, fast CPU inference via ONNX Runtime

## Download

Install with [Homebrew](https://brew.sh/):

```bash
brew install --cask damione1/tap/souffle
```

Or grab a prebuilt installer from the [**Releases**](https://github.com/damione1/souffle/releases/latest) page: a `.dmg` for Apple Silicon Macs, requiring macOS 13 or newer.

## Build from source

Requires an Apple Silicon Mac, [Rust](https://rustup.rs/), [Node.js](https://nodejs.org/) 18+, and [cmake](https://cmake.org/) (`brew install cmake`).

```bash
npm install
npm run tauri dev
```

## License

Copyright (c) 2026 Damien Goehrig.

Released under the GNU General Public License v3.0 or later (GPL-3.0-or-later). You are free to use, study, modify, and redistribute this software, provided that derivative works are also published under the same license. See [LICENSE.md](LICENSE.md) for the full text.
