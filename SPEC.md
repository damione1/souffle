# Project Codename: Voxtral (placeholder)

## Local Speech-to-Text Desktop Application - Technical Specification

**Version:** 1.0.1-draft
**Author:** Damien (Lead Data Engineer / Software Engineer, 10+ years)
**Date:** 2026-03-19
**Target:** macOS (Apple Silicon M1-M4), cross-platform ready by design

---

## 1. Vision & Goals

### What this is
A polished, privacy-first desktop application for local speech-to-text transcription. Two primary modes:
1. **Dictation mode**: Push-to-talk or toggle recording. Via global shortcut: transcribed text is copied to clipboard and auto-pasted (Cmd+V) into the active application. Via UI button: text is copied to clipboard only (no simulated paste).
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
| IPC contract generation | specta + tauri-specta | latest compatible | Rust-first DTO, command, and event bindings exported to TypeScript |

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
| Audio capture | cpal | 0.15.x | Microphone capture on macOS; system audio via BlackHole virtual device workaround |
| Resampling | rubato | 0.15+ | High-quality resample to 24kHz mono f32 (Mimi/Kyutai requirement) |
| Audio I/O | hound | 3.5+ | WAV debug capture / offline inspection |
| VAD / end-of-turn | Kyutai semantic VAD heads | via moshi | Built-in end-of-turn scoring from the model |

### System Integration
| Component | Technology | Purpose |
|-----------|-----------|---------|
| Global hotkeys | tauri-plugin-global-shortcut | Toggle recording from anywhere |
| System tray | Tauri TrayIconBuilder | Menu bar presence, status indicator |
| Tray positioning | tauri-plugin-positioner | Optional positioning of compact windows/popovers near the tray |
| Notifications | tauri-plugin-notification | Transcription complete, errors |
| Auto-start | tauri-plugin-autostart | Launch at login (optional) |
| Single instance | tauri-plugin-single-instance | Prevent multiple instances |
| Local storage | SQLite (rusqlite) | Meetings, dictation, settings, search, embeddings |
| File system | tauri-plugin-fs | Model storage, recordings |
| Auto-updater | tauri-plugin-updater | GitHub releases based updates |
| Logging | tauri-plugin-log | Structured logging for debugging |

### LLM Integration (Meeting Summarization)
| Component | Technology | Purpose |
|-----------|-----------|---------|
| LLM runtime | Ollama (external) | Local LLM inference via HTTP API (localhost:11434) |
| Communication | reqwest | HTTP client for Ollama API |
| Model management (future) | ollama-rs | Stream `/api/pull`, inspect local tags, surface download progress |
| Summary model | User-selected chat/instruction model | Recommended: qwen, llama, mistral, gemma, phi, deepseek; speech/embedding models excluded |

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

### 3.1.1 IPC Contract Strategy

**Rust is the source of truth for all frontend/backend contracts.**

- All IPC-facing DTOs and enums derive `serde::{Serialize, Deserialize}` and `specta::Type`
- Tauri commands are registered through `tauri-specta` so command names, arguments, and return types are exported from Rust
- App-level events (`navigate`, shortcut events) are exported as typed event contracts, not stringly typed frontend listeners
- Streaming payloads continue to use Tauri `Channel<T>`, but the payload DTOs are still Rust-defined and exported to TypeScript
- Generated bindings are committed in the frontend:
  - `src/lib/types/generated.ts` for DTOs, command signatures, and events
  - `src/lib/api/generated.ts` as the thin typed frontend entrypoint
- `src/lib/types/index.ts` remains a re-export layer only; manual duplication of IPC DTOs is not allowed

**Contract rules:**

- Settings persistence uses one typed DTO (`AppSettings`), including `audio_device`
- Engine/model/runtime metadata must be exposed via typed descriptors, never ad hoc JSON maps
- New scalable surfaces must be introduced behind DTOs or interfaces first, especially for:
  - multiple STT engines/models
  - summarization providers and model catalogs
  - streamed progress/status payloads

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

**Current audio queue design:**
- `crossbeam-channel::bounded` carries session-tagged `AudioChunk` values from capture to inference
- Audio is resampled inline to 24kHz mono f32 before enqueue
- Each chunk is tagged with a monotonically increasing `session_id`
- The capture callback only emits when its `active_session_id` matches the current session
- Start/stop paths drain stale chunks to keep session boundaries clean

### 3.4 Model Management

Models are NOT embedded in the binary. They are downloaded on first run.

```
$APP_DATA_DIR/
  models/
    kyutai/
      stt-1b-en_fr/
        model.safetensors
        tokenizer.model
        config.json
        mimi-pytorch.safetensors
  souffle.db                  (meetings, segments, dictation history, settings, FTS)
  debug_engine_input.wav      (optional, only when transcription debug is enabled)
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
- Input: Raw audio (PCM f32, 24kHz mono)
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
- [x] Mimi codec expected sample rate = 24kHz
- [ ] Streaming behavior: how to feed continuous audio vs file-based
- [ ] Whether the stt-rs example is production-ready or just a demo

### 3.6 Current Implementation Assessment (2026-03-19)

**Overall status**
- The core application is operational end-to-end: model download/load, dictation, meeting recording, SQLite persistence, live transcript streaming, and Ollama summarization.
- The backend architecture has held up well: a dedicated audio thread feeds a dedicated inference thread, while Tauri commands and channels keep UI control and streaming updates simple.
- The Kyutai integration is viable for interactive use on Apple Silicon, but it still needs longer-run profiling and automated soak testing before calling it production-hardened.

**Most important issue that was fixed**
- The main stability issue was a session-boundary bug that looked like a memory leak: the first transcription session worked, but the second session could crash, reuse stale audio, or get stuck in an invalid decoding state.
- Root cause was not a single leak. It was a combination of:
  - late CoreAudio/cpal callbacks still enqueueing audio after `Stop`
  - stale audio chunks crossing session boundaries
  - reset/rebuild of the Kyutai/Candle Metal state happening too aggressively on reused resources
- The fix was layered:
  - audio chunks are now tagged with a `session_id`
  - the capture callback is gated by an `active_session_id` atomic and becomes a no-op immediately after stop
  - stale chunks are drained/ignored on stop/start boundaries
  - session start/reset is serialized around the persistent inference pipeline
  - Kyutai reset now synchronizes the Metal device and rebuilds the ASR state/device cleanly
- Current result: multiple back-to-back dictation sessions and meeting-recording sessions were validated manually without reproducing the previous second-session crash/leak symptom.

**Other implementation improvements completed during troubleshooting**
- Meeting recording now shows live transcript feedback in the Recordings tab while recording, instead of only after stop.
- Verbose transcription diagnostics are runtime-gated behind a `debug_transcription` setting rather than always-on logs.
- Meeting summarization is now stricter and more extractive:
  - speech/embedding models such as `whisper` are filtered out for summaries
  - the prompt explicitly forbids invented decisions, action items, and greeting-style intros
  - users can re-run summarization on an existing meeting with a different model

**Current technical limitations**
- System audio capture is still not true native CoreAudio loopback; the practical workaround is selecting BlackHole as the input device.
- The session-boundary fix is validated manually, not yet by an automated regression or soak test.
- Summary quality still depends heavily on using a proper text-generation model in Ollama.

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
2. Audio capture starts from the currently selected input device (microphone, or BlackHole if the user wants system audio)
3. Audio is streamed through the same persistent Kyutai inference pipeline used by dictation
4. Live transcript segments are shown in the Recordings view while the meeting is active
5. Tray icon shows "recording meeting" state (pulsing indicator)
6. Final segments are accumulated in memory during the meeting
7. User stops recording via tray menu or hotkey (default: Cmd+Shift+M)
8. On stop, the meeting transcript is saved to SQLite (`meetings` + `segments` tables)
9. If Ollama is available with a summary-capable text model, the user can summarize or re-summarize the meeting
10. Summary, model name, and generation timestamp are persisted on the meeting row

**Current persisted meeting shape (logical model):**
- `id`
- `title`
- `started_at`
- `ended_at`
- `duration_seconds`
- `engine`
- `segments[]`
- `summary`
- `summary_model`
- `summary_generated_at`

### 4.3 Ollama Summarization

**Prerequisites:** Ollama running locally (user's responsibility to install)
**Detection:** On app startup and periodically, check `GET http://localhost:11434/api/tags`
**Flow:**
1. After meeting transcription is complete, check if Ollama is available
2. Filter installed Ollama tags to summary-capable text-generation models only
3. In meeting detail view, show a model picker plus `Summarize` / `Re-summarize`
4. On click, send the transcript to Ollama with a strict extractive prompt:
   - no greeting, no intro, no invented decisions or action items
   - fixed markdown structure: `Summary`, `Decisions`, `Action Items`, `Topics`
   - if the transcript does not state something, say so explicitly instead of inferring
5. Stream response back to UI
6. Save summary, model name, and timestamp in SQLite

**Ollama API call:**
- Endpoint: `POST http://localhost:11434/api/generate`
- Model: user-configurable summary-capable text model (not STT / embedding models)
- Stream: true (show summary generation in real-time)
- Temperature: `0.0` to minimize drift and hallucination

### 4.4 Meeting Detection (Future)

There is no single reliable cross-app meeting API on macOS. The recommended implementation is a **multi-signal decision pipeline** ordered by privacy impact and reliability:

1. **Process + network activity (`sysinfo` + connection inspection)**  
   Detect Zoom / Teams / Slack / Meet companion processes, then confirm active meeting-related network connections. This is the highest-value zero-permission signal.
2. **CoreAudio device state (`coreaudio-rs`)**  
   Watch `kAudioDevicePropertyDeviceIsRunningSomewhere` to learn that the microphone is in use without reading audio data.
3. **Calendar prediction (`objc2-event-kit`)**  
   Parse Zoom / Teams / Meet URLs from current or upcoming calendar events to provide "meeting starting soon" hints.
4. **Window title detection (`winshift`, optional)**  
   Use Accessibility permission to inspect meeting window titles for stronger confirmation when needed.

**Recommended decision logic**
- If meeting app process is running and it has active meeting-network connections: **meeting detected**
- Else if microphone device is active and a meeting app process is running: **meeting detected**
- Else if a calendar event with a meeting URL is happening now: **probable meeting**

Platform-specific OAuth integrations (Zoom webhooks, Teams Graph presence, Slack events) should remain optional add-ons, not the primary local-detection path.

### 4.5 Speaker Diarization (Future)

The preferred architecture is **post-processing diarization**, not a replacement of the existing Kyutai STT loop:

1. Run Kyutai STT as-is to generate timestamped segments
2. In parallel, resample the same 24kHz mono recording to **16kHz mono** with `rubato`
3. Feed the 16kHz stream/file to **`pyannote-rs`** (preferred), which combines segmentation and speaker embeddings via ONNX Runtime
4. Align diarization spans with transcript timestamps
5. Insert speaker labels / paragraph breaks where the active speaker changes

**Recommended library choice**
- `pyannote-rs` is the default recommendation for v1 diarization because it is already proven in a Tauri desktop app and integrates cleanly as a separate inference pass.
- `native-pyannote-rs`, `sherpa-rs`, and `parakeet-rs` remain evaluation options, but they are not the primary path today.

---

## 5. UI Design

### 5.1 App Window

The main window is a **sidebar-based single-page app** (~1100x700, min 800x500) with four views navigated via a left sidebar (~200px, icons + text labels).

**Design system ("Private Oracle"):** Dark glassmorphism first, Manrope (headings) + Inter (body) self-hosted fonts, ghost borders (`1px solid rgba(170, 171, 176, 0.15)`), glass panels (`backdrop-filter: blur(24px)`), 0.75rem default roundness. Light theme supported via toggle.

**Sidebar collapses** to icon-only (~72px) below 800px viewport width.

**Waveform footer:** A canvas-based animated waveform bar sits at the bottom of the content area (full-width, ~40px). Idle: subtle ambient sine wave. Active: bars driven by real audio RMS level from the backend (`get_audio_level` command, polled at ~20Hz). The audio capture thread computes RMS per chunk and stores it in a shared `AtomicU32`.

**Transcription view (default):**
- Model gate: download/load section (shown conditionally when model not ready)
- Capability badges (engine name, language support, auto-paste status)
- Large circular record button with animated states (ready/starting/recording)
- Live transcript scrollable text area
- Input device + output mode indicators
- **Auto-paste behavior**: shortcut-triggered stop → copies to clipboard + simulates Cmd+V (if auto-paste enabled). Button-triggered stop → copies to clipboard only, no Cmd+V. Anti-duplicate guard (`isStopping` flag) prevents multiple paste events from key repeat.
- **Dictation history**: flat list, always visible when entries exist. Each entry shows text clamped to 5 lines (CSS `line-clamp`), click to expand full text. Inline Copy/Delete buttons per entry. "Clear all" requires confirmation dialog.

**Meeting view (two sub-states):**
- **New meeting form**: title input + "Start Recording" button. Pressing Enter or clicking starts recording and transitions to the meeting item page.
- **Meeting item page** (active recording or viewing saved meeting):
  - Header: meeting title, status pill (Recording / Completed), back button (to history), stop/new meeting actions
  - **Key insights grid**: after summary generation, bullet points from the summary are extracted and displayed as numbered cards above the two-column layout
  - Two-column layout:
    - Left: transcript with paragraph grouping (word-level segments joined into flowing paragraphs, timestamp `[MM:SS]` at the start of each paragraph, new paragraph on pause > 1.5s + sentence-ending punctuation)
    - Right: summary panel — empty state with icon when no summary, "Generate Summary" CTA button with model selector, streaming output during generation, model attribution pill after completion
  - Footer: delete meeting with confirmation
- Navigation: `app.currentMeetingId` in store drives which meeting is displayed. `app.openMeeting(id)` navigates from history, `app.newMeeting()` resets to form.

**Meeting History view:**
- "Meeting History" header with count badge
- Client-side search bar filtering on meeting titles
- Flat list of clickable meeting cards (not accordions) — each card shows title, date, duration, summary badge, chevron indicator
- Clicking a card navigates to the meeting item page via `app.openMeeting(id)`
- Empty state message

**Settings view:**
- Audio Configuration: input device selector + refresh button, noise reduction toggle (grayed out, "Coming Soon" badge)
- Intelligence: Kyutai STT shown as single read-only entry, Ollama URL input + connection status dot + retry button, summarization model dropdown (all text-gen models from Ollama)
- Interface: theme buttons (Dark/Light/System), auto-paste toggle + delay input (shown conditionally), global shortcuts (toggle recording + push-to-talk with keyboard capture UI, Mac symbol formatting)
- Diagnostics: debug transcription logs toggle
- About: version v0.1.0, engine info, privacy statement

**Removed from original design:** Sampling rate selector (fixed at 24kHz), disk usage block, DB encryption status, bottom audio player/playback controls, engine selection dropdown (only Kyutai implemented), accordion-style meeting expansion in history

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

**Implementation notes**
- Use Tauri v2's built-in `TrayIconBuilder` and runtime `set_icon()`; do not define a duplicate tray icon in both `tauri.conf.json` and code.
- Bundle icon variants at compile time with `include_bytes!`.
- Use a template icon for idle/ready state so macOS can adapt it to dark/light menu bar themes.
- Use a non-template colored icon for active recording state.
- Important gotcha: `set_icon()` resets the template flag, so call `set_icon_as_template(true)` again after swapping back to a template icon.
- Use 22×22px tray assets (44×44px at `@2x` for Retina).
- Hide the main window on close and prevent process exit when all windows are closed so recording can continue from the tray.

### 5.3 UI Guidelines

- **Minimalist.** The app should feel invisible when not in use.
- **Dark mode first** (matches macOS developer aesthetic), light mode supported.
- **No onboarding wizard.** First launch: download model, done.
- **Animations:** subtle transitions only. No gratuitous motion.
- **Typography:** Manrope (headings, 600-800), Inter (body, 400-600), self-hosted WOFF2.
- **Colors:** Canvas `#0c0e12`, surfaces `#111318`→`#23262c`, accents blue `#4e8eff` / violet `#a78bfa` / teal `#2dd4bf`. Ghost border overlays for depth.

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

### 6.4 Autostart / Login Items

**Recommended implementation**
- Use `tauri-plugin-autostart` v2 with the Builder API and pass `--autostarted` to distinguish login launches from user launches.
- Expose autostart as an explicit user-controlled setting. Do not enable it silently.

**macOS-specific caveats**
- On Ventura+ the app appears in System Settings → General → Login Items, where users can disable it.
- Unsigned apps are shown as background items from an unidentified developer, so code signing + notarization are effectively mandatory for a good experience.
- LaunchAgent plist files can outlive app uninstall and leave orphan entries behind.
- For a more native macOS-only path later, evaluate `smappservice-rs` / `SMAppService`, which integrates more cleanly with Login Items and uninstall behavior.

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
- [x] Meeting transcript storage and history view (later migrated from JSON to SQLite in Phase 4)
- [x] Ollama integration for meeting summarization (streaming)
- [x] Settings UI: audio device, auto-paste, Ollama config, theme
- [x] Dark/light/system mode theming
- [x] Tab-based navigation (Dictation / Recordings / Settings)
- [x] Enhanced tray menu (Start/Stop Dictation, Meeting Recording, Settings)

**Implementation notes:**
- Auto-paste uses `arboard` (clipboard) + `enigo` (Cmd+V simulation) — requires macOS Accessibility permission
- Meeting recording exposes live transcript updates in the Recordings tab during capture
- Ollama integration: `POST /api/generate` with streaming NDJSON
- Summary UI supports re-running with a different model on an already summarized meeting
- Tray behavior should remain code-driven via `TrayIconBuilder`; use template icons for idle state and a colored icon for recording state
- Theme: CSS class strategy (`.light`/`.dark` on `<html>`), Tailwind overrides in app.css
- Frontend restructured: shared types in `src/lib/types/`, reactive store in `src/lib/stores/`
- ASR Word emission: emit immediately on `Word` event (don't wait for `EndWord` which has 5s+ latency)
- Inter-word spaces added in frontend (SentencePiece strips leading `▁` when decoding per-word)
- Paragraph breaks inserted on pause > 1.5s after sentence-ending punctuation
- Runtime debug logs are gated behind a persisted `debug_transcription` toggle
- Session boundaries are isolated with session-tagged audio chunks and callback-level gating

### Phase 4: Unified Storage & Search ✅ (Steps 5-6 complete, vector search pending)
- [x] SQLite unified storage (`souffle.db` via rusqlite with bundled-full/FTS5)
- [x] Meeting CRUD via SQLite (replaces JSON file I/O)
- [x] JSON → SQLite auto-migration (existing meetings imported on first run)
- [x] Dictation history backend (replaces frontend-only tauri-plugin-store)
- [x] Settings migration to SQLite key-value table (tauri-plugin-store removed entirely)
- [x] FTS5 full-text search across meetings and dictation
- [x] Transcript review/edit before summarization
- [ ] Vector embeddings + semantic search

**Implementation notes:**
- DB path: `~/Library/Application Support/com.souffle.app/souffle.db`
- WAL mode + foreign keys enabled, schema versioned via `schema_version` table
- `Database` struct with `Mutex<Connection>`, all methods take `&self`
- JSON migration is idempotent — existing meeting IDs checked before insert, dirs renamed to `.json_backup`
- Settings stored as JSON-encoded values in `settings(key, value)` table
- `tauri-plugin-store` fully removed from Cargo.toml, package.json, and capabilities
- FTS5 `text_search` virtual table populated on meeting save and dictation entry add
- Schema includes `embeddings` table for future vector search (Step 7)
- Current storage of record summaries is `meetings.summary`, `meetings.summary_model`, `meetings.summary_generated_at`
- Debug and session-isolation work landed on top of the SQLite-backed architecture without changing the external UI flow

**Roadmap (future sessions):**

#### Step 5: Full-text search ✅
- [x] FTS5 search across meetings and dictation entries
- [x] `search_text(query)` command returning snippets with `<mark>` highlighting
- [x] Search bar UI above meeting list in Recordings tab
- [x] Search bar UI in dictation history section
- [x] Debounced FTS5 search (250ms) with highlighted snippets in cards
- [x] Schema v4 migration: contentless FTS5 → content-storing, re-indexes all data
- [x] Query: `SELECT snippet(text_search, 0, '<mark>', '</mark>', '...', 32), source_type, source_id, rank FROM text_search WHERE text_search MATCH ? ORDER BY rank LIMIT 20`

#### Step 6: Transcript review/edit before summarization ✅
- [x] `edited_transcript TEXT` column on meetings (schema v4)
- [x] "Review & Edit" pencil button on transcript section → textarea pre-filled with segment text
- [x] "Save & Summarize" / "Save" / "Cancel" actions
- [x] `summarize_meeting` uses `edited_transcript` if present, else joins segments
- [x] "Edited" badge when transcript has been edited
- [x] "Reset to original" button clears `edited_transcript` back to null

#### Step 7: Vector embeddings + semantic search
- [ ] `EmbeddingProvider` trait with `embed(texts) -> Vec<Vec<f32>>`, `dimensions()`, `model_name()`
- [ ] Ollama primary: `POST /api/embed` with `nomic-embed-text` model
- [ ] On-device fallback: Candle sentence transformer (all-MiniLM-L6-v2, ~80MB)
- [ ] Chunk meeting transcript into ~200-word chunks with 50-word overlap
- [ ] Store `Vec<f32>` as BLOB via `f32::to_le_bytes()`
- [ ] Cosine similarity search in Rust (load all embeddings, compute, sort, return top-K)
- [ ] Generate embeddings after `stop_meeting_recording` or on-demand via button

#### Step 8: Meeting detection pipeline
- Process detection via `sysinfo`
- Confirm active calls with process-scoped network activity checks
- Add CoreAudio device-state monitoring via `coreaudio-rs`
- Add EventKit meeting-URL prediction via `objc2-event-kit`
- Optionally add Accessibility-based window-title confirmation via `winshift`
- Keep app-specific OAuth integrations optional, not required

#### Step 9: Ollama model pull UX
- Add in-app Ollama model list / pull flow
- Preferred crate: `ollama-rs` with streaming pull support
- Stream pull progress to the frontend via Tauri Channels, not unordered events
- Show manifest, layer-download, digest verification, and completion states
- Never use a short read timeout for large pulls; only use a connect timeout

### Phase 5: Distribution (Target: 1 week)
- [ ] Apple Developer Program enrollment ($99/year — required for signing + notarization)
- [ ] Code signing (Developer ID certificate) + notarization pipeline (GitHub Actions, `xcrun notarytool`)
- [ ] DMG installer with custom icon
- [ ] Auto-updater via GitHub releases (`tauri-plugin-updater`)
- [ ] Autostart toggle via `tauri-plugin-autostart` with Ventura+ Login Items validation
- [ ] Verify login-item behavior for signed/notarized builds and document uninstall cleanup
- [ ] README, website/landing page

**Distribution strategy: Direct website + notarized DMG (not App Store)**

App Store is not viable for Souffle — sandboxing restrictions conflict with core features:
- System audio capture (BlackHole virtual device workaround blocked in sandbox)
- Accessibility permission for auto-paste (`enigo` Cmd+V simulation) — gray area in App Store review
- Global shortcuts outside the app window
- Localhost network access to Ollama (`localhost:11434`) needs entitlement justification
- 30% Apple cut on a one-time purchase indie app is prohibitive

Direct distribution with notarized DMG gives full system access, no review gatekeeping, and ~5% payment fees vs 30%. Notarization ensures no Gatekeeper warnings — users double-click and it works. This is the standard for pro Mac tools (Raycast, CleanShot, etc.).

**Licensing & monetization strategy:**

- **Payment + license keys**: Lemon Squeezy or Polar.sh (handles payment, tax compliance, key generation, download hosting, ~5-8% fee)
- **Activation**: Hybrid offline-signed keys with one-time online activation
  1. User purchases → receives license key
  2. First launch → enters key → app calls serverless validation endpoint → stores signed activation token locally (Ed25519)
  3. Subsequent launches → validates local token offline (public key embedded in binary)
  4. Grace period: 30 days without re-check if offline
- **Revocation**: Online activation enables device fingerprinting and key revocation if needed
- **Server**: Single serverless function (Cloudflare Worker / Vercel edge function, ~50 lines)
- **Rust crates**: `ed25519-dalek` for token signing/verification
- **Anti-piracy posture**: Don't over-invest in DRM. Rust compiled binary + code signing + signed activation token is more protection than 90% of indie Mac apps. The audience (professionals) buys; the niche is too small for crack groups. Gate features that involve real data flow (unlimited history, export, summary) rather than boolean flags.
- **Feature gating ideas**: Free tier = core STT. Paid = unlimited meeting history, transcript editing, summary generation, export, search

### Phase 6: Multi-Engine & Meeting Intelligence (v1.5+)

#### Step 1 — Per-engine audio requirements ✅
- [x] `TranscriptionEngine::audio_requirements()` trait method returns `AudioInputRequirements`
- [x] Pipeline `active_loop` reads `chunk_size_samples` from engine (replaces hardcoded `MIMI_FRAME_SIZE`)
- [x] `Resampler::new(source_rate, channels, target_rate)` — configurable target rate per engine
- [x] `AudioCommand::Start { session_id, target_sample_rate }` — audio thread reconfigures per engine

#### Step 2 — Whisper engine via whisper-rs ✅
- [x] `whisper-rs` 0.16 dependency with `metal` + `tracing_backend` features
- [x] `engine/whisper.rs` — `WhisperEngine` implementing `TranscriptionEngine` trait
- [x] Batch-oriented: accumulates PCM internally, transcribes on 5s chunk boundary or flush
- [x] GGML model loading (auto-detects `.bin` in model directory)
- [x] Catalog updated: Whisper turbo `available_in_app: true`, backend `whisper-rs`
- [x] Artifact: `ggerganov/whisper.cpp` / `ggml-large-v3-turbo.bin` (~1.6GB)
- [x] `create_engine()` factory routes `(whisper, whisper-rs)` → `WhisperEngine`

#### Step 3 — Engine switching in settings without restart ✅
- [x] State machine already supports `Unload { next_profile }` → `Loading` → `Ready`
- [x] `load_model` command detects profile change and triggers unload→reload cycle
- [x] Audio thread reconfigures sample rate on each recording session start

#### Step 4 — Parakeet engine via ort crate (v2, future)
- [ ] `ort` crate dependency (ONNX Runtime with CoreML backend)
- [ ] `engine/parakeet.rs` — `ParakeetEngine` implementing `TranscriptionEngine`
- [ ] Catalog activation: Parakeet TDT-CTC 1.1B model, `nemo` backend
- [ ] ONNX model artifacts from NVIDIA NeMo collection on HuggingFace

#### Step 5 — Speaker diarization via pyannote-rs
- [ ] `DiarizationEngine` trait in `engine/diarization.rs`
- [ ] `SpeakerSegment { speaker_id, start_time, end_time, label }` DTO
- [ ] `speaker_id: Option<String>` field on `TranscriptionSegment`
- [ ] DB schema v5: `speaker_id` column on `segments` table
- [ ] `PyAnnoteEngine` implementation using `pyannote-rs` (ONNX-based)
- [ ] Post-processing: run after meeting stop, align speakers to text segments by timestamp
- [ ] `enable_diarization: bool` setting, audio buffer retention during meeting recording

#### Step 6 — Meeting detection pipeline
- [ ] `meeting_detection/mod.rs` module with background polling thread (5s interval)
- [ ] Signal sources: `sysinfo` (process list: Zoom/Teams/Meet/FaceTime), CoreAudio device state, EventKit calendar
- [ ] `MeetingDetectionEvent` DTO with `DetectionSource` enum
- [ ] Tauri event emission on meeting start/end detection
- [ ] Notification + auto-record option
- [ ] `enable_meeting_detection: bool` and `detection_apps: Vec<String>` settings

#### Step 7 — Tray-positioned mini window / popover
- [ ] `tauri-plugin-positioner` dependency
- [ ] Small window (~300×200) positioned near tray icon
- [ ] Content: recording status, mini waveform, start/stop button
- [ ] `tray_mini_window: bool` setting

---

## 9. Risks & Mitigations

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| Candle Metal perf on M4 Pro insufficient for real-time 1B model | Critical | Medium | **Validate in Phase 2 ASAP.** Fallback: Whisper via whisper-rs (proven). Candle Metal is less optimized than whisper.cpp Metal. |
| Kyutai stt-rs is demo-quality, not production-ready | High | Medium | Budget extra time to harden. Isolate in engine trait so replacement is cheap. |
| Native CoreAudio loopback / system-audio capture on macOS remains fragile | High | Low | Current workaround is BlackHole as the selected input device. Longer-term fallback: ScreenCaptureKit or a different capture backend. |
| ~~Multi-session transcription leaked/contaminated audio between sessions~~ | ~~Critical~~ | ~~High~~ | ✅ **Resolved on 2026-03-19**: session-tagged audio chunks, callback-level `active_session_id` gating, stale-queue draining, serialized reset/start ordering, and clean Kyutai/Metal state rebuild. Add soak tests later. |
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
      db/
        mod.rs             # SQLite database entry point
        schema.rs          # DB schema + migrations
        migrate.rs         # JSON -> SQLite migration
        meetings.rs        # Meeting persistence
        dictation.rs       # Dictation history persistence
        settings.rs        # Settings persistence
      engine/
        mod.rs             # Engine module + TranscriptionEngine trait
        kyutai.rs          # Kyutai STT implementation
      models/
        mod.rs             # Model download, verification, management
        download.rs        # HuggingFace download with progress
      pipeline/
        mod.rs             # Persistent audio -> engine -> output pipeline
      ollama/
        mod.rs             # Ollama HTTP client
      commands.rs          # All #[tauri::command] functions
      state.rs             # AppState, shared state management
      transcript.rs        # Shared meeting transcript structs
      debug.rs             # Runtime debug toggles
      clipboard.rs         # Copy/paste integration
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
    App.svelte             # Main single-page shell
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
