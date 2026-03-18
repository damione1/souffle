# Project Codename: Voxtral (placeholder)

## Local Speech-to-Text Desktop Application - Technical Specification

**Version:** 1.0.0-draft
**Author:** Damien (Lead Data Engineer / Software Engineer, 10+ years)
**Date:** 2026-03-17
**Target:** macOS (Apple Silicon M1-M4), cross-platform ready by design

---

## 1. Vision & Goals

### What this is
A polished, privacy-first desktop application for local speech-to-text transcription. Two primary modes:
1. **Dictation mode**: Push-to-talk or toggle recording, transcribed text is pasted into the active application (like SuperWhisper/VoxDrop)
2. **Meeting recording mode**: Continuous background recording of system audio (Teams/Zoom/Meet calls), saved for post-meeting transcription and LLM-based summarization via Ollama

### What this is NOT
- Not a cloud service. Everything runs locally. Zero data leaves the machine.
- Not an Electron app. No embedded Chromium. Tauri v2 with native WebView.
- Not a multi-process nightmare. Single binary, single installer, no terminal commands required.

### Key differentiators
- **Kyutai STT as primary engine** - First desktop app to ship with Kyutai's streaming STT model. Bilingual French/English (stt-1b-en_fr) with 500ms latency. True token-by-token streaming, not 30-second chunk processing.
- **Multi-engine architecture** - Abstracted engine trait allows swapping/adding engines (Whisper, Parakeet, Kyutai) without refactoring
- **French-first** - Designed for bilingual fr/en users from day one
- **Meeting recording + summarization** - Not just dictation, but full meeting capture with local LLM integration
- **One-time purchase model** - No subscriptions for local computation

### Developer context
- Senior software engineer, 10+ years experience (PHP/Symfony, Go, DevOps, CI/CD)
- No prior Rust or desktop app experience
- Power user of Claude Code (treat this as the primary development tool)
- Daily driver: MacBook Pro M4 Pro
- This spec is designed to be fed directly to Claude Code for implementation

---

## 2. Technology Stack

### Core
| Component | Technology | Version | Purpose |
|-----------|-----------|---------|---------|
| App framework | Tauri | v2 (stable) | Desktop app shell, IPC, system integration |
| Backend language | Rust | latest stable | Application logic, ML inference, audio processing |
| Frontend framework | Svelte | v5 (runes) | Minimal UI in WebView (compiles to vanilla JS) |
| Frontend styling | Tailwind CSS | v4 | Utility-first, minimal CSS output |
| Frontend language | TypeScript | v5 | Type safety for frontend |

### STT Inference
| Component | Technology | Purpose |
|-----------|-----------|---------|
| Primary engine | Kyutai STT (stt-1b-en_fr) | Bilingual FR/EN streaming transcription |
| Inference runtime | Candle (Rust) | Native Rust ML inference framework by HuggingFace |
| Model format | Candle/safetensors | Native format for Kyutai models |
| Fallback engine (v1.5) | Whisper large-v3-turbo | Via whisper-rs, broader language support |
| Future engine (v2) | Parakeet TDT 0.6B v3 | Via ort crate (ONNX Runtime), 25 EU languages |

### Audio
| Component | Technology | Version | Purpose |
|-----------|-----------|---------|---------|
| Audio capture | cpal | 0.17+ | Microphone + system audio (CoreAudio loopback on macOS 14.6+) |
| Resampling | rubato | 0.15+ | High-quality resample to 16kHz mono f32 (Whisper/Kyutai requirement) |
| Audio I/O | hound | 3.5+ | WAV file read/write for meeting recordings |
| VAD | Silero VAD | ONNX via ort | Voice Activity Detection (avoid transcribing silence) |

### System Integration
| Component | Technology | Purpose |
|-----------|-----------|---------|
| Global hotkeys | tauri-plugin-global-shortcut | Toggle recording from anywhere |
| System tray | Tauri TrayIconBuilder | Menu bar presence, status indicator |
| Notifications | tauri-plugin-notification | Transcription complete, errors |
| Auto-start | tauri-plugin-autostart | Launch at login (optional) |
| Single instance | tauri-plugin-single-instance | Prevent multiple instances |
| Local storage | tauri-plugin-store | User preferences, engine config |
| File system | tauri-plugin-fs | Model storage, recordings |
| Auto-updater | tauri-plugin-updater | GitHub releases based updates |
| Logging | tauri-plugin-log | Structured logging for debugging |

### LLM Integration (Meeting Summarization)
| Component | Technology | Purpose |
|-----------|-----------|---------|
| LLM runtime | Ollama (external) | Local LLM inference via HTTP API (localhost:11434) |
| Communication | reqwest | HTTP client for Ollama API |
| Default model | User's choice | Recommended: llama3.1:8b or mistral |

---

## 3. Architecture

### 3.1 High-Level Architecture

```
+----------------------------------------------------------+
|                    Tauri v2 Application                   |
|                                                           |
|  +-------------------+     IPC/Channel     +-----------+  |
|  |   Rust Backend    | <=================> |  Svelte   |  |
|  |                   |    (sub-ms latency)  |  Frontend |  |
|  |  +-------------+  |                     |           |  |
|  |  | AudioManager|  |  --- events ------> | Status UI |  |
|  |  +------+------+  |                     | Settings  |  |
|  |         |          |  --- channel -----> | History   |  |
|  |  +------v------+  |   (stream segments) | Controls  |  |
|  |  | EngineManager|  |                     +-----------+  |
|  |  +------+------+  |                                    |
|  |         |          |                                    |
|  |  +------v------+  |                                    |
|  |  | Transcription|  |                                    |
|  |  | Pipeline     |  |                                    |
|  |  +------+------+  |                                    |
|  |         |          |                                    |
|  |  +------v------+  |                                    |
|  |  | OllamaClient|  |  (HTTP to localhost:11434)         |
|  |  +-------------+  |                                    |
|  +-------------------+                                    |
+----------------------------------------------------------+
```

### 3.2 Engine Abstraction (Critical Design Decision)

All STT engines MUST implement a common Rust trait. This is the foundation for multi-engine support.

```rust
/// Core trait that ALL transcription engines must implement.
/// Adding a new engine (Whisper, Parakeet, Kyutai, future models)
/// means implementing this trait - nothing else changes.
pub trait TranscriptionEngine: Send + Sync {
    /// Human-readable engine name for UI display
    fn name(&self) -> &str;

    /// Supported languages as ISO 639-1 codes
    fn supported_languages(&self) -> Vec<String>;

    /// Whether this engine supports true streaming (token-by-token)
    /// vs chunk-based processing (e.g., Whisper's 30s windows)
    fn supports_streaming(&self) -> bool;

    /// Initialize the engine, load model weights into memory.
    /// Called once at startup or when user switches engines.
    /// `model_path`: absolute path to model weights on disk.
    /// `device`: target compute device (CPU, Metal GPU, etc.)
    fn load_model(&mut self, model_path: &Path, device: Device) -> Result<(), EngineError>;

    /// Unload model from memory. Called when switching engines
    /// or shutting down. Must free all GPU/CPU memory.
    fn unload_model(&mut self) -> Result<(), EngineError>;

    /// Process an audio chunk and return transcription segments.
    /// `audio`: raw PCM f32 samples at 16kHz mono.
    /// `language`: optional language hint (None = auto-detect).
    /// Returns zero or more segments (streaming engines may return
    /// partial/non-final segments).
    fn transcribe(
        &self,
        audio: &[f32],
        language: Option<&str>,
    ) -> Result<Vec<TranscriptionSegment>, EngineError>;

    /// For streaming engines: signal that audio input has ended.
    /// Returns any remaining buffered segments.
    /// Non-streaming engines can return Ok(vec![]).
    fn flush(&self) -> Result<Vec<TranscriptionSegment>, EngineError>;

    /// Estimated VRAM/RAM usage in bytes for the loaded model.
    /// Used by UI to warn users about memory pressure.
    fn memory_usage(&self) -> Option<u64>;
}

/// A piece of transcribed text with metadata
#[derive(Debug, Clone, serde::Serialize)]
pub struct TranscriptionSegment {
    pub text: String,
    pub start_time: f64,       // seconds from recording start
    pub end_time: f64,         // seconds from recording start
    pub is_final: bool,        // false = may be revised by engine
    pub language: Option<String>, // detected language
    pub confidence: Option<f32>,  // 0.0-1.0 if available
}

/// Errors that engines can produce
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("Model not found at path: {0}")]
    ModelNotFound(PathBuf),
    #[error("Failed to load model: {0}")]
    LoadError(String),
    #[error("Inference failed: {0}")]
    InferenceError(String),
    #[error("Unsupported language: {0}")]
    UnsupportedLanguage(String),
    #[error("Engine not initialized")]
    NotInitialized,
    #[error("Out of memory")]
    OutOfMemory,
}
```

### 3.3 Threading Model

**CRITICAL: Never run inference on the tokio async runtime.**

```
Main thread (tokio)     - Tauri event loop, IPC, UI commands
Audio thread (std)      - cpal audio capture callback, ring buffer write
Inference thread (std)  - Engine.transcribe() calls, CPU/GPU bound
Resampling (inline)     - rubato runs in audio thread before buffer write

Communication:
  Audio thread --[crossbeam-channel]--> Inference thread
  Inference thread --[tauri::ipc::Channel]--> Frontend (streaming segments)
  Frontend --[tauri::command]--> Main thread (start/stop/config)
```

**Ring buffer design for audio:**
- Capacity: 30 seconds of audio at 16kHz = 480,000 f32 samples (~1.8 MB)
- Audio thread writes continuously
- Inference thread reads in chunks (configurable: 1-5 seconds for streaming, 30s for Whisper)
- If buffer overflows (inference too slow), log warning and drop oldest samples
- Use `crossbeam-channel::bounded` with backpressure

### 3.4 Model Management

Models are NOT embedded in the binary. They are downloaded on first run.

```
$APP_DATA_DIR/
  models/
    kyutai/
      stt-1b-en_fr/
        model.safetensors
        tokenizer.json
        config.json
    whisper/                  (v1.5)
      large-v3-turbo-q5_0.bin
    parakeet/                 (v2)
      tdt-0.6b-v3.onnx
  recordings/
    2026-03-17_standup.wav
    2026-03-17_standup.json   (transcription + metadata)
  config.json                 (user preferences)
```

**Download flow:**
1. First launch: detect no models present
2. Show model selection UI with sizes and descriptions
3. Download from HuggingFace with progress bar (via Tauri Channel API)
4. Verify checksum (SHA-256)
5. Store in app data directory
6. Models persist across app updates

### 3.5 Kyutai STT Integration Details

The Kyutai STT model uses the Delayed Streams Modeling framework.
Reference implementation: https://github.com/kyutai-labs/delayed-streams-modeling

**Key technical details:**
- Audio encoder: Mimi neural codec, encodes audio at 12.5 Hz into 32 tokens/frame
- Architecture: Decoder-only Transformer
- Model: stt-1b-en_fr (1B params, bilingual, ~2GB weights)
- Input: Raw audio (PCM f32, 24kHz for Mimi - resample from 16kHz if needed, check Mimi's expected sample rate)
- Output: Token stream with word-level timestamps, punctuation, capitalization
- Latency: ~500ms for the 1B model
- Built-in Semantic VAD: predicts end-of-utterance from content, not just silence

**Rust integration path:**
- Use the Candle-based inference code from `stt-rs/` in the delayed-streams-modeling repo
- Key dependency: `candle-core`, `candle-nn`, `candle-transformers`
- Metal acceleration via `candle-core` feature flag `metal`
- The model runs a streaming loop: feed audio frames, get text tokens back
- Reference: `delayed-streams-modeling/stt-rs/src/main.rs`

**IMPORTANT: Verify these assumptions early in development:**
- [ ] Candle Metal performance on M4 Pro for 1B param model
- [ ] Actual latency in practice (claimed 500ms)
- [ ] Memory footprint (expected ~4-6 GB with model loaded)
- [ ] Mimi codec expected sample rate (may be 24kHz, not 16kHz)
- [ ] Streaming behavior: how to feed continuous audio vs file-based
- [ ] Whether the stt-rs example is production-ready or just a demo

---

## 4. Application Modes & User Flows

### 4.1 Dictation Mode

**Trigger:** Global hotkey (default: Cmd+Shift+Space) or tray icon click
**Flow:**
1. User presses hotkey -> tray icon changes to "recording" state (red dot)
2. Audio capture starts from default microphone
3. Audio is streamed to Kyutai engine in real-time
4. Transcribed text appears in a floating overlay near cursor (optional, can be disabled)
5. User presses hotkey again -> recording stops
6. Final transcription is copied to clipboard AND pasted into active app
7. Tray icon returns to idle state

**Clipboard integration:**
- Use `arboard` crate for cross-platform clipboard access
- For paste-into-active-app on macOS: use `enigo` crate to simulate Cmd+V
- Respect a configurable delay before paste (default: 100ms) to ensure clipboard is ready

### 4.2 Meeting Recording Mode

**Trigger:** Manual start from app window or tray menu
**Flow:**
1. User selects "Start Meeting Recording" from tray menu or app window
2. Audio capture starts from system audio (CoreAudio loopback via cpal 0.17)
3. Optionally, also capture microphone (dual-stream for speaker separation in v2)
4. Audio is saved to WAV file in real-time (hound crate)
5. Tray icon shows "recording meeting" state (pulsing indicator)
6. User can optionally see live transcription in app window
7. User stops recording via tray menu or hotkey (default: Cmd+Shift+M)
8. Post-processing: full transcription saved as JSON alongside WAV
9. If Ollama is available: offer to summarize the meeting transcript
10. Summary saved alongside transcript

**Meeting transcript JSON format:**
```json
{
  "id": "uuid-v4",
  "created_at": "2026-03-17T14:30:00Z",
  "duration_seconds": 3600,
  "engine": "kyutai-stt-1b-en_fr",
  "language_detected": "fr",
  "audio_file": "2026-03-17_standup.wav",
  "segments": [
    {
      "text": "Bonjour tout le monde, on commence le standup.",
      "start_time": 0.0,
      "end_time": 3.2,
      "is_final": true,
      "language": "fr",
      "confidence": 0.94
    }
  ],
  "summary": null,
  "summary_model": null,
  "summary_generated_at": null
}
```

### 4.3 Ollama Summarization

**Prerequisites:** Ollama running locally (user's responsibility to install)
**Detection:** On app startup and periodically, check `GET http://localhost:11434/api/tags`
**Flow:**
1. After meeting transcription is complete, check if Ollama is available
2. If available, show "Summarize" button in meeting detail view
3. On click, send transcript to Ollama with a system prompt:
   - "You are a meeting summarizer. Given the following meeting transcript, produce:
     1. A concise summary (2-3 paragraphs)
     2. Key decisions made
     3. Action items with responsible persons (if identifiable)
     4. Topics discussed
     Respond in the same language as the transcript."
4. Stream response back to UI
5. Save summary in the transcript JSON

**Ollama API call:**
- Endpoint: `POST http://localhost:11434/api/generate`
- Model: user-configurable (default: auto-detect first available model)
- Stream: true (show summary generation in real-time)

---

## 5. UI Design

### 5.1 App Window

The main window is a **compact, single-page app** with three tabs/views:

**Dictation view (default):**
- Large central microphone button (toggle recording)
- Current engine indicator (e.g., "Kyutai STT 1B - FR/EN")
- Live transcription text area (read-only, shows current session)
- Language indicator (auto-detected)
- "Copy last" button

**Recordings view:**
- List of past meeting recordings (date, duration, engine used)
- Click to expand: full transcript, summary (if generated), audio playback
- "Summarize" button (if Ollama available and no summary yet)
- "Export" button (copy transcript, export as .txt/.srt)
- "Delete" button (with confirmation)

**Settings view:**
- Engine selection (dropdown: Kyutai, Whisper [if installed], Parakeet [if installed])
- Model management (download/delete models, show sizes)
- Global hotkey configuration
- Dictation: auto-paste on/off, paste delay
- Audio: input device selection, system audio capture on/off
- Ollama: connection URL, preferred model
- General: launch at login, language preference, theme (light/dark/system)
- About: version, licenses, links

### 5.2 System Tray

**Icon states:**
- Idle: monochrome microphone icon
- Dictation recording: red microphone icon
- Meeting recording: pulsing red dot
- Processing: animated spinner (brief, during post-processing)
- Error: yellow warning badge

**Tray menu:**
- "Start Dictation" (or "Stop Dictation" if active)
- "Start Meeting Recording" (or "Stop Recording" if active)
- Separator
- "Show Window"
- "Settings"
- Separator
- "Quit"

### 5.3 UI Guidelines

- **Minimalist.** The app should feel invisible when not in use.
- **Dark mode first** (matches macOS developer aesthetic), light mode supported.
- **No onboarding wizard.** First launch: download model, done.
- **Animations:** subtle transitions only. No gratuitous motion.
- **Typography:** system font (-apple-system / SF Pro on macOS).
- **Colors:** monochrome with a single accent color for recording state.

---

## 6. macOS Distribution

### 6.1 Requirements
- Apple Developer Program ($99/year) - required for notarization
- Developer ID Application certificate
- DMG installer (Tauri generates this natively)

### 6.2 Code Signing & Notarization

**Entitlements required (Info.plist / entitlements.plist):**
```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <!-- Required for Tauri WebView JIT -->
    <key>com.apple.security.cs.allow-jit</key>
    <true/>
    <key>com.apple.security.cs.allow-unsigned-executable-memory</key>
    <true/>
    <key>com.apple.security.cs.allow-dyld-environment-variables</key>
    <true/>

    <!-- Microphone access -->
    <key>com.apple.security.device.audio-input</key>
    <true/>

    <!-- Screen/system audio capture (for meeting recording) -->
    <key>com.apple.security.device.audio-recording</key>
    <true/>

    <!-- Network access (for model download + Ollama) -->
    <key>com.apple.security.network.client</key>
    <true/>

    <!-- File access (recordings, models) -->
    <key>com.apple.security.files.user-selected.read-write</key>
    <true/>
</dict>
</plist>
```

**CI/CD:** GitHub Actions with `tauri-apps/tauri-action` for automated build, sign, notarize, and release.

### 6.3 Known Gotchas
- **DO NOT use UPX compression** - breaks macOS code signing
- Notarization upload can stall 15-20 min silently. Use `xcrun notarytool history` to check.
- CoreML cold-start penalty is ~15 min on first run per device. Start with Metal only.
- Binary size target: ~20-30 MB (without models). Models are separate downloads.
- Cargo release profile must include: `strip = true`, `lto = true`, `codegen-units = 1`, `opt-level = "s"`, `panic = "abort"`

---

## 7. Licensing & Attribution

### Project license
MIT (permissive, matches Kyutai code license and Whisper)

### Third-party licenses to comply with
| Dependency | License | Obligation |
|-----------|---------|------------|
| Kyutai STT model weights | CC-BY 4.0 | Attribution required in About screen and README |
| Kyutai STT Rust code | Apache 2.0 | Include license notice |
| Whisper (future) | MIT | Include license notice |
| Parakeet (future) | CC-BY 4.0 | Attribution required |
| Tauri | MIT/Apache 2.0 | Include license notice |
| Candle | MIT/Apache 2.0 | Include license notice |

**Attribution text for About screen:**
"Speech recognition powered by Kyutai STT (kyutai.org), licensed under CC-BY 4.0.
Built with Tauri, Candle, and open-source technologies."

---

## 8. Milestones

### Phase 1: Foundation ✅ (Completed 2026-03-17)
- [x] Scaffold Tauri v2 project with Svelte 5 + TypeScript + Tailwind
- [x] Implement system tray with icon states and basic menu
- [x] Implement global hotkey registration (Cmd+Shift+Space)
- [x] Audio capture from microphone via cpal (24kHz mono f32, updated from 16kHz in Phase 2)
- [x] Ring buffer + crossbeam-channel audio pipeline
- [x] Save captured audio to WAV file (validate audio pipeline works)

**Implementation notes:**
- Stack: Vite 7.3 / @sveltejs/vite-plugin-svelte 6 / Tailwind CSS 4 / Svelte 5 / Tauri 2.10 / Rust 1.94
- cpal `Stream` is `!Send` on macOS → AudioCapture runs on a dedicated `std::thread`, communicates via `AudioCommand` channel
- Audio pipeline: cpal callback → Resampler (rubato, mono 24kHz f32) → crossbeam bounded(30) → drain on stop
- Global hotkey emits `recording-started`/`recording-stopped` Tauri events to sync frontend
- WAV files saved to `~/Library/Application Support/com.souffle.app/recordings/`
- `TranscriptionEngine` trait defined in `engine/mod.rs`, ready for Phase 2
- Using cpal 0.15 (not 0.17) — loopback capture deferred to Phase 3

### Phase 2: Kyutai Integration ✅ (Completed 2026-03-17)
- [x] Integrate Candle with Metal feature flag
- [x] Port/adapt Kyutai stt-rs inference code into the app
- [x] Model download manager (HuggingFace, progress bar, checksum)
- [x] Implement KyutaiEngine: TranscriptionEngine trait
- [x] Streaming transcription pipeline: audio -> engine -> frontend via Channel API
- [ ] Benchmark: latency, memory, accuracy on M4 Pro (pending runtime validation)

**Implementation notes:**
- Audio pipeline changed from 16kHz to 24kHz — Mimi codec confirmed to require 24kHz input
- Uses `moshi` crate v0.6.1 (provides Mimi codec, LM model, ASR state machine) — no raw porting needed
- Candle 0.9.x with `metal` feature for Apple GPU; `candle-nn`, `candle-transformers` at same version
- SentencePiece tokenizer (not HuggingFace `tokenizers` crate) — requires cmake for native build
- `KyutaiEngine` wraps `moshi::asr::State` — streaming: 1920 PCM samples (80ms) → Mimi encode → LM forward → text tokens
- Model download via `hf-hub` with symlinks to HF cache to avoid doubling ~2.4GB on disk
- `TranscriptionPipeline` runs on dedicated `std::thread`, buffers audio into 1920-sample frames
- New Tauri commands: `get_model_status`, `download_model`, `load_model`, `start_transcription`, `stop_transcription`
- Frontend: model download UI → load model button → streaming transcription via Channel API
- VAD extra heads enabled (4 heads at 0.5s/1s/2s/3s horizons) for future end-of-turn detection

**Important** If there is any kind of issue with versions or lib config, compatibility, etc... alway check the documentation via context7 or internet if not available on context7.

### Phase 3: Polish & Features (Target: 2 weeks) ✅ COMPLETE
- [x] Dictation mode: auto-paste via clipboard + Cmd+V simulation
- [x] Meeting recording mode: mic recording with live transcription (system audio deferred — BlackHole workaround documented)
- [x] Meeting transcript storage (JSON) and history view
- [x] Ollama integration for meeting summarization (streaming)
- [x] Settings UI: audio device, auto-paste, Ollama config, theme
- [x] Dark/light/system mode theming
- [x] Tab-based navigation (Dictation / Recordings / Settings)
- [x] Enhanced tray menu (Start/Stop Dictation, Meeting Recording, Settings)

**Implementation notes:**
- Auto-paste uses `arboard` (clipboard) + `enigo` (Cmd+V simulation) — requires macOS Accessibility permission
- Meeting transcripts stored as JSON in `~/Library/Application Support/com.souffle.app/meetings/{uuid}.json`
- Ollama integration: `POST /api/generate` with streaming NDJSON, system prompt from SPEC
- Settings persisted via `tauri-plugin-store` (`settings.json`)
- Theme: CSS class strategy (`.light`/`.dark` on `<html>`), Tailwind overrides in app.css
- Frontend restructured: shared types in `src/lib/types/`, reactive store in `src/lib/stores/`
- ASR Word emission: emit immediately on `Word` event (don't wait for `EndWord` which has 5s+ latency)
- Inter-word spaces added in frontend (SentencePiece strips leading `▁` when decoding per-word)
- Paragraph breaks inserted on pause > 1.5s after sentence-ending punctuation
- Shutdown sequence: stop audio → wait 300ms → stop pipeline (drains channel) → flush engine

### Phase 4: Distribution (Target: 1 week)
- [ ] Apple Developer Program enrollment
- [ ] Code signing + notarization pipeline (GitHub Actions)
- [ ] DMG installer with custom icon
- [ ] Auto-updater via GitHub releases
- [ ] README, website/landing page

### Phase 5: Multi-Engine (v1.5, future)
- [ ] Whisper engine via whisper-rs (Metal acceleration)
- [ ] Engine switching in settings without restart
- [ ] Parakeet engine via ort crate (v2)
- [ ] Speaker diarization (v2)

---

## 9. Risks & Mitigations

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| Candle Metal perf on M4 Pro insufficient for real-time 1B model | Critical | Medium | **Validate in Phase 2 ASAP.** Fallback: Whisper via whisper-rs (proven). Candle Metal is less optimized than whisper.cpp Metal. |
| Kyutai stt-rs is demo-quality, not production-ready | High | Medium | Budget extra time to harden. Isolate in engine trait so replacement is cheap. |
| cpal 0.17 CoreAudio loopback unstable | High | Low | Fallback: ScreenCaptureKit via objc2 bindings, or require BlackHole virtual audio device. |
| ~~Mimi codec sample rate mismatch~~ | ~~Medium~~ | ~~Medium~~ | ✅ **Resolved in Phase 2**: Confirmed 24kHz. Pipeline updated from 16kHz to 24kHz. |
| macOS notarization rejects binary with Candle Metal shaders | Medium | Low | Test notarization early in Phase 2, not Phase 4. |
| Model download size (2GB) deters users | Medium | Medium | Show clear progress, allow background download, offer smaller model if Kyutai releases one. |

---

## 10. Development Guidelines for Claude Code

### Repo structure
```
voxtral/
  src-tauri/
    src/
      main.rs              # Tauri app entry point
      lib.rs               # Re-exports
      audio/
        mod.rs             # Audio module exports
        capture.rs         # cpal microphone + system audio capture
        resampler.rs       # rubato wrapper
        ring_buffer.rs     # Lock-free ring buffer
      engine/
        mod.rs             # Engine module + TranscriptionEngine trait
        kyutai.rs          # Kyutai STT implementation
        whisper.rs         # (v1.5) Whisper implementation
        parakeet.rs        # (v2) Parakeet implementation
      models/
        mod.rs             # Model download, verification, management
        download.rs        # HuggingFace download with progress
      pipeline/
        mod.rs             # Orchestrates audio -> engine -> output
        dictation.rs       # Dictation mode logic
        meeting.rs         # Meeting recording mode logic
      ollama/
        mod.rs             # Ollama HTTP client
        summarize.rs       # Meeting summarization prompts
      commands.rs          # All #[tauri::command] functions
      state.rs             # AppState, shared state management
      config.rs            # User configuration, persistence
      tray.rs              # System tray setup and event handling
      errors.rs            # App-wide error types
    Cargo.toml
    tauri.conf.json
    build.rs
    icons/
    entitlements.plist
  src/                     # Svelte frontend
    lib/
      components/          # Svelte components
      stores/              # Svelte stores (state management)
      types/               # TypeScript types matching Rust structs
    routes/                # Pages (if using SvelteKit-like routing)
    app.html
    app.css
  package.json
  svelte.config.js
  vite.config.ts
  tailwind.config.ts
  README.md
  LICENSE
  SPEC.md                 # This file
```

### Coding conventions
- **Rust:** Follow standard Rust idioms. Use `thiserror` for error types. Use `tracing` for logging.
- **No unwrap() in production code.** Use `?` operator and proper error propagation.
- **All Tauri commands must return Result<T, String>** (Tauri's IPC serialization requirement).
- **Frontend types must mirror Rust structs.** Generate TypeScript types from Rust using `ts-rs` crate or manual sync.
- **Comments:** Focus on WHY, not WHAT. Code should be self-documenting.
- **Tests:** Unit tests for engine trait implementations, audio pipeline. Integration tests for model loading.

### Claude Code workflow
1. Start with `src-tauri/src/engine/mod.rs` - define the trait first
2. Scaffold the audio pipeline (`audio/` module) and validate with WAV output
3. Integrate Kyutai - this is the riskiest part, do it early
4. Build the Tauri commands layer (`commands.rs`)
5. Build the Svelte frontend last - the backend should work headlessly first
6. Polish, test, distribute

### AGENTS.md / CLAUDE.md
Create an AGENTS.md at repo root with:
- Build commands: `cargo tauri dev`, `cargo tauri build`
- Test commands: `cargo test --manifest-path src-tauri/Cargo.toml`
- Key architectural decisions (engine trait, threading model)
- Known gotchas (Candle Metal, cpal loopback, notarization)
- Link to this SPEC.md
