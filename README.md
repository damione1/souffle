# Souffle

A private, local speech-to-text desktop app for macOS. Everything runs on your machine — no cloud, no API keys, no data leaves your computer.

## What it does

- **Dictation** — Press a button or a global shortcut, speak, and get text. Optionally auto-pastes into whatever app you were using.
- **Meeting transcription** — Record a meeting with a live transcript that streams word by word.
- **Meeting summaries** — Generate summaries from your transcripts using a local Ollama model.
- **Full-text search** — All transcripts and dictation entries are indexed and searchable.

## Speech models

All models run locally and are downloaded on first use from HuggingFace:

- [Kyutai STT 1B](https://huggingface.co/kyutai/stt-1b-en_fr-candle) (default) — French + English streaming transcription, ~2.4 GB, Metal GPU via Candle
- [Kyutai STT 2.6B](https://huggingface.co/kyutai/stt-2.6b-en-candle) — English, higher quality, ~5.6 GB
- [Whisper Large V3 Turbo](https://huggingface.co/ggerganov/whisper.cpp) — multilingual, ~1.6 GB, Metal via whisper.cpp
- [Parakeet TDT 0.6B v3](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx) — 25 languages with punctuation and capitalization, ~670 MB int8, fast CPU inference via ONNX Runtime

## Prerequisites

- macOS with Apple Silicon (M1/M2/M3/M4,M5)
- [Rust](https://rustup.rs/) toolchain
- [Node.js](https://nodejs.org/) (v18+)
- [cmake](https://cmake.org/) (`brew install cmake`) — required to build the tokenizer
- [Ollama](https://ollama.com/) (optional) — for meeting summaries

## Getting started

```bash
# Install frontend dependencies
npm install

# Run in development mode
npm run tauri dev
```

On first launch, the app will prompt you to download and load the speech model (~2.4 GB). This is a one-time step.

## Building for production

```bash
npm run tauri build
```

The `.dmg` installer is output to `src-tauri/target/release/bundle/dmg/`.

## Running tests

```bash
# Rust tests
cargo test --manifest-path src-tauri/Cargo.toml

# Frontend tests
npm test
```

## License

Private project.
