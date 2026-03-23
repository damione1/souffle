# Souffle

A private, local speech-to-text desktop app for macOS. Everything runs on your machine — no cloud, no API keys, no data leaves your computer.

## What it does

- **Dictation** — Press a button or a global shortcut, speak, and get text. Optionally auto-pastes into whatever app you were using.
- **Meeting transcription** — Record a meeting with a live transcript that streams word by word.
- **Meeting summaries** — Generate summaries from your transcripts using a local Ollama model.
- **Full-text search** — All transcripts and dictation entries are indexed and searchable.

## Speech model

Souffle uses [Kyutai STT 1B](https://huggingface.co/kyutai/stt-1b-en_fr-candle) (French + English), a ~2.4 GB model that runs locally with Metal GPU acceleration on Apple Silicon. The model is downloaded on first launch from HuggingFace.

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
cargo tauri dev
```

On first launch, the app will prompt you to download and load the speech model (~2.4 GB). This is a one-time step.

## Building for production

```bash
cargo tauri build
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
