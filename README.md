# Soufflé

![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)
![Tauri 2](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white)
![Svelte 5](https://img.shields.io/badge/Svelte-5-FF3E00?logo=svelte&logoColor=white)
![TypeScript](https://img.shields.io/badge/TypeScript-3178C6?logo=typescript&logoColor=white)
![macOS Apple Silicon](https://img.shields.io/badge/platform-macOS%20Apple%20Silicon-lightgrey)

A private, local speech-to-text app for macOS. Everything runs on-device: no cloud, no API keys, nothing leaves your machine.

## What it does

- **Dictation**, with auto-paste into whatever app you were using and a global shortcut to start it from anywhere.
- **Meeting transcription**, with a live transcript and system-audio capture that separates Me from Them.
- **Meeting summaries**, generated from your transcripts by a local Ollama model.
- **Full-text search** across every transcript and dictation entry.

## Speech models

All models run locally and are downloaded on first use from HuggingFace:

- [Kyutai STT 1B](https://huggingface.co/kyutai/stt-1b-en_fr-candle) (default): French + English streaming transcription, ~2.4 GB, Metal GPU via Candle
- [Kyutai STT 2.6B](https://huggingface.co/kyutai/stt-2.6b-en-candle): English, higher quality, ~5.6 GB
- [Whisper Large V3 Turbo](https://huggingface.co/ggerganov/whisper.cpp): multilingual, ~1.6 GB, Metal via whisper.cpp
- [Parakeet TDT 0.6B v3](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx): 25 languages with punctuation and capitalization, ~670 MB int8, fast CPU inference via ONNX Runtime

## Download

Prebuilt installers are on the [**Releases**](https://github.com/damione1/souffle/releases/latest) page: a `.dmg` for Apple Silicon Macs, requiring macOS 13 or newer.

## Getting started

### Prerequisites

- An Apple Silicon Mac
- [Rust](https://rustup.rs/) toolchain
- [Node.js](https://nodejs.org/) 18+
- [cmake](https://cmake.org/) (`brew install cmake`), required to build the tokenizer
- [Ollama](https://ollama.com/) (optional), for meeting summaries

### Develop

```bash
npm install
npm run tauri dev
```

### Build

```bash
npm run tauri build
```

Tagged releases (`git tag vX.Y.Z && git push origin vX.Y.Z`) are built, signed, and notarized by CI.

### Tests

```bash
# Rust tests
cargo test --manifest-path src-tauri/Cargo.toml

# Frontend tests
npm test
```

## Tech stack

[Tauri 2](https://tauri.app/) (Rust core) · [SvelteKit](https://kit.svelte.dev/) + [Svelte 5](https://svelte.dev/) · TypeScript. Inference runs via [Candle](https://github.com/huggingface/candle) (Metal), whisper.cpp, and ONNX Runtime.

## License

Released under the MIT License.
