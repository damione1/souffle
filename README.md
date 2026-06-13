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

Without a signing identity the bundle is only ad-hoc signed: it runs on the
build machine, but any other Mac will quarantine it on arrival (AirDrop,
download…) and Gatekeeper reports it as *"damaged and can't be opened"*.
Recipients can bypass that with `xattr -cr /Applications/Soufflé.app`, but the
real fix is signing + notarization.

### Signing & notarization (distribution)

Requires an [Apple Developer Program](https://developer.apple.com/programs/)
membership and a **Developer ID Application** certificate installed in the
keychain (Xcode → Settings → Accounts → Manage Certificates, or
developer.apple.com → Certificates). Check it with:

```bash
security find-identity -v -p codesigning
```

Then build with the signing/notarization environment set — Tauri picks these
up automatically and staples the notarization ticket to the DMG:

```bash
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export APPLE_ID="you@example.com"            # Apple ID of the developer account
export APPLE_PASSWORD="app-specific-pwd"      # appleid.apple.com → App-Specific Passwords
export APPLE_TEAM_ID="TEAMID"
npm run tauri build
```

The hardened runtime is enabled with the entitlements in
`src-tauri/entitlements.plist` (JIT/unsigned-memory exceptions required by the
Metal inference runtimes).

## Running tests

```bash
# Rust tests
cargo test --manifest-path src-tauri/Cargo.toml

# Frontend tests
npm test
```

## License

Private project.
