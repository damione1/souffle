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
| Third engine (v2, shipped) | Parakeet TDT 0.6B v3 int8 | Via parakeet-rs + ort load-dynamic (shared bundled ONNX Runtime), 25 languages incl. FR/EN, punctuation/capitalization, CPU |

### Audio
| Component | Technology | Version | Purpose |
|-----------|-----------|---------|---------|
| Audio capture | cpal | 0.15.x | Microphone capture on macOS |
| System audio capture | objc2-core-audio | 0.3.x | Core Audio process tap (macOS 14.4+) — native capture of all process output, no virtual device |
| Echo cancellation | sonora | 0.1.x | Pure-Rust WebRTC AEC3; cancels speaker leakage from the mic during meetings |
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
///
/// OWNERSHIP MODEL: engines are created, used, swapped, and dropped
/// exclusively on the engine actor thread (see 3.3). The trait therefore
/// has NO Send/Sync bound and methods take &mut self — engines need no
/// interior locking, and !Send inference types (e.g. parakeet-rs's
/// ParakeetTDT) are usable. Product metadata (labels, languages,
/// capabilities) lives in the catalog descriptors, not on the runtime.
pub trait TranscriptionEngine {
    /// Load model weights into memory from the given directory.
    fn load_model(&mut self, model_path: &Path) -> Result<(), EngineError>;

    /// Unload model from memory. Must free all GPU/CPU memory.
    fn unload_model(&mut self) -> Result<(), EngineError>;

    /// Process an audio chunk and return transcription segments.
    /// Audio arrives resampled to `audio_requirements().sample_rate_hz`.
    fn transcribe(
        &mut self,
        audio: &[f32],
        language: Option<&str>,
    ) -> Result<Vec<TranscriptionSegment>, EngineError>;

    /// Signal that audio input has ended; return remaining buffered segments.
    fn flush(&mut self) -> Result<Vec<TranscriptionSegment>, EngineError>;

    /// Reset per-session state (called before each recording session).
    fn reset_state(&mut self) -> Result<(), EngineError>;

    /// Sample rate / channels / chunk size this engine expects; drives
    /// audio capture resampling and pipeline framing.
    fn audio_requirements(&self) -> AudioInputRequirements;

    /// Gain factor applied to raw microphone input before inference.
    fn mic_gain(&self) -> f32 { 1.0 }

    /// Strip engine-specific tokens (SentencePiece ▁, Whisper [_TT_], …).
    fn normalize_text(&self, text: &str) -> String { text.to_string() }
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

### 3.3 Threading Model — Engine Actor

**CRITICAL: Never run inference on the tokio async runtime.**

```
Main thread (tokio)        - Tauri event loop, IPC, UI commands
Audio thread (std)         - cpal capture callback, resample, enqueue
Engine actor thread (std)  - OWNS the engine: create/load/transcribe/swap/drop

Communication:
  Audio thread --[crossbeam bounded(512) AudioMessage]--> Engine actor
  Commands --[crossbeam unbounded EngineCommand + bounded(1) replies]--> Engine actor
  Engine actor --[tauri::ipc::Channel]--> Frontend (streaming segments)
  Engine actor --[tauri-specta events]--> Frontend (TranscriptionHealth, PipelineError)
  Frontend --[tauri::command]--> Main thread (start/stop/config)
```

**Engine actor (`pipeline/actor.rs`):** spawned once at startup, lives for the
app's lifetime. All engine lifecycle happens on this one thread, driven by
`EngineCommand::{LoadModel, UnloadModel, StartSession, StopSession, …}` with
bounded(1) reply channels. This is what makes native-library teardown safe:
sentencepiece (Kyutai) and ONNX Runtime (VAD/Parakeet, dlopen'ed via ort
load-dynamic) never see cross-thread create/drop ordering, and autorelease
pools live in exactly two places (per-command/per-session in the actor,
per-frame inside engines).

**Audio queue & deterministic stop:**
- `AudioMessage::Chunk` carries session-tagged samples plus `captured_at` (lag tracking)
- The capture callback `try_send`s; on a full channel it drops the chunk and
  increments a shared counter surfaced in health events — capture never blocks
- On stop, the audio thread drops the cpal stream (synchronous), flushes the
  resampler's partial chunk, then sends `AudioMessage::EndOfStream` as the
  guaranteed-last message; the actor finishes the session when that marker
  arrives (no sleeps, no guess timeouts — a 15s reply timeout remains as a
  last-resort safety net and stop failures never wedge the state machine)
- Stale chunks/markers from previous sessions are filtered by `session_id`
  and drained at session start

**Health & failure surfacing:** during a session the actor emits
`TranscriptionHealth` (~1/s: queue depth, chunk lag, dropped chunks,
Healthy/Lagging/Stalled) and `PipelineError` events. A single transcribe
failure skips the frame; 25 consecutive failures abort the session, stop
audio capture, and fail the state machine — the pipeline can no longer die
silently while the UI looks like it is still recording.

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
- The session-boundary fix is validated manually, not yet by an automated regression or soak test.
- Summary quality still depends heavily on using a proper text-generation model in Ollama.

### 3.7 Native system-audio capture for meetings (2026-06-12)

Meeting mode now captures two audio sources with zero user configuration — no
more Audio MIDI Setup aggregate devices or BlackHole:

- **Microphone**: unchanged cpal capture.
- **System audio**: a mono global Core Audio process tap (macOS 14.4+,
  `CATapDescription` + `AudioHardwareCreateProcessTap`) wrapped in a private,
  programmatically created aggregate device whose IOProc delivers the mixed
  output of all processes. Requires the "System Audio Recording Only" TCC
  permission (`NSAudioCaptureUsageDescription` in `Info.plist`); the prompt
  fires on first tap creation. Denial or any tap failure degrades gracefully
  to mic-only and surfaces a `SystemAudioStatus` event to the UI.

**Topology** (`audio/mixer.rs`): both real-time callbacks (cpal, tap IOProc)
only push raw samples into lock-free SPSC rings. While a meeting is active the
audio thread's command loop switches from blocking `recv()` to a 5ms
`recv_timeout()` tick that drives `MeetingMixer`: resample both legs to 48kHz,
pair into 10ms frames (mic paces; missing tap = zeros), optional AEC, sum +
clamp, resample to the engine rate, and forward `AudioMessage::Chunk` exactly
as before — the engine actor and EndOfStream drain contract are untouched.
Dictation keeps the original direct-callback path.

**Echo cancellation** (`audio/aec.rs`): when the default output routes to the
built-in speakers (transport `BuiltIn` + data source `'ispk'`), the tap leg is
fed to a WebRTC AEC3 (`sonora`) as the render signal and the mic leg is
processed as capture, so remote participants played out loud are not
transcribed twice. With headphones the AEC is skipped entirely. The meeting
tick re-checks the output route every ~2s, rebuilding the tap when the default
output device changes and toggling AEC on speaker/headphone switches. Tap
drift against the mic clock is bounded by dropping tap lead beyond 250ms.

### 3.8 Engine Actor Refactor (2026-06-11)

**Why:** adding the second engine (Whisper) and the Silero VAD filter exposed
two failure classes. (1) The pipeline could silently stop transcribing: the
bounded audio channel filled when inference lagged real-time and audio was
dropped without signal, and a single transcribe error killed the inference
thread while the UI still looked like it was recording. (2) Protobuf
double-free crashes between sentencepiece (static protobuf) and ONNX
Runtime — fixed by ort `load-dynamic`, but the shared
`Arc<Mutex<Box<dyn TranscriptionEngine>>>` (created/used/dropped on different
threads with manual ordering) kept the hazard alive for every new engine.

**What changed:**
- Engine ownership moved into a single **engine actor thread** (section 3.3);
  `Arc<Mutex<engine>>`, the per-load pipeline respawn, and the fragile
  swap/drop ordering in `load_model` were deleted
- `TranscriptionEngine` lost its `Send + Sync` bound and interior Mutexes;
  methods take `&mut self`
- Stop/drain became event-ordered via an `EndOfStream` marker (no more
  300ms sleep + 5s drain timeout); the resampler's final partial chunk is
  flushed instead of discarded
- Pipeline health (lag/queue/drops) and errors are emitted to the frontend;
  sessions abort visibly after repeated engine failures
- Parakeet TDT 0.6B v3 implemented on the shared dynamic ONNX Runtime,
  proving the third-engine path the refactor was designed for

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
2. Audio capture starts from the currently selected input device, plus a native system-audio tap (Core Audio process tap, macOS 14.4+) mixed in — capturing Teams/Zoom output with zero configuration
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
- [x] Meeting recording mode: mic recording with live transcription (native system-audio capture added 2026-06-12 via Core Audio process tap)
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
- System audio capture via Core Audio process taps (TCC-gated, hostile to sandbox review)
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

#### Whisper Integration — Lessons Learned

The whisper-rs/whisper.cpp integration required solving several non-obvious issues:

**`detect_language=true` is detect-only mode (critical)**
In whisper.cpp, `params.detect_language = true` runs language detection then **returns immediately without transcribing** (`return 0` at line 6824). This is by design (PR #853) — it's a CLI feature for querying a file's language. The correct way to auto-detect AND transcribe is `set_language(None)` (null pointer), which triggers the same detection code path but does NOT early-return. The whisper-rs docs are misleading: they say `set_detect_language` has "the same effect" as `set_language(None)` — it does not.

**Language caching for streaming chunks**
Auto-detection on every 5-second chunk causes hallucinations (decoder fills silence with counting sequences like "44, 44, 44...") and repetition loop failures requiring temperature fallback (0.0 → 0.2 → 0.4 → 0.6). Fix: auto-detect on first chunk, cache the result in `detected_language: Mutex<Option<String>>`, use explicit language for all subsequent chunks. Reset on `reset_state()`.

**`print_special` affects segment text, not just console output**
In whisper.cpp (line 7615), `print_special` controls what tokens go into `result_all[].text` returned by the API: `if (params.print_special || tokens_cur[i].id < whisper_token_eot(ctx))`. With `print_special=false`, only word tokens are stored. This is correct for our use — `normalize_text()` handles any remaining special tokens.

**`single_segment=true` required for short chunks**
Without it, the decoder generates EOT immediately on 5-second audio. With explicit language + `single_segment=true`, decoding is stable. With `language=None` + `single_segment=true`, decoding works after the detect_language fix above.

**Mic gain abstraction**
Kyutai previously used 15x mic gain to compensate for low audio levels. This clipped Whisper audio. Added `mic_gain()` to the `TranscriptionEngine` trait — both engines now return 1.0 (Kyutai quality actually improved with 1x gain).

**Engine hot-swap on restart**
`load_model` checked machine state profile to skip `create_engine()`, but the machine state was just set while the actual engine was still the previous one (Kyutai). Fix: always create a fresh engine on `load_model`.

**Metal cleanup on exit**
Without explicit engine unload before exit, `ggml_metal_rsets_free` assertion fires (SIGABRT). Fix: `.build().run(|app, event|)` pattern with `ExitRequested` handler that unloads engine and shuts down pipeline.

#### Step 4 — Parakeet engine (shipped 2026-06-11)
- [x] `parakeet-rs 0.3.6` (default-features off, `load-dynamic`) sharing the bundled ONNX Runtime dylib (upgraded to 1.24.4 for ort api-24)
- [x] `engine/parakeet.rs` — `ParakeetEngine` implementing `TranscriptionEngine` (16kHz, 5s windows, CPU int8; CoreML is slower than CPU for these dynamic-shape graphs)
- [x] Catalog activation: **TDT 0.6B v3** (`istupakov/parakeet-tdt-0.6b-v3-onnx`, int8, ~670MB), `onnx-ort` backend — the original TDT-CTC 1.1B target has no ONNX export anywhere and is English-only/older-generation, so the multilingual v3 replaced it
- [x] Verified by real inference on synthesized EN and FR speech (punctuation + capitalization confirmed)

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

### Phase 7A: Pipeline Quality (Immediate Fixes)

Whisper transcription quality in Souffle is notably worse than competing apps using the same model. The primary culprit is our primitive energy-based VAD (lets noise through → hallucinations) and lack of text post-processing. These steps directly improve transcription results.

#### Step 1 — Silero VAD v5 (Replace Energy VAD)

**Problem**: `has_speech()` in whisper.rs uses RMS energy threshold (0.00005). Too primitive — lets background noise through causing Whisper hallucinations, rejects quiet speech.

**Solution**: Silero VAD v5 neural model with smoothing wrapper.

**Crate**: `voice-activity-detector` 0.2.1 (bundles Silero v5 ONNX ~2MB, uses `ort` runtime)
- Frame: 512 samples at 16kHz (32ms)
- Sub-millisecond latency on CPU (no GPU needed)
- 6000+ language support

**Architecture**:
- [ ] New file: `src-tauri/src/pipeline/vad.rs` — `SmoothedVad` wrapper
- [ ] SmoothedVad: onset ~14 frames (~450ms), hangover ~14 frames (~450ms), prefill ~14 frames
- [ ] Runs in pipeline `active_loop` BEFORE `engine.transcribe()`
- [ ] For 16kHz engines (Whisper): direct passthrough to VAD
- [ ] For 24kHz engines (Kyutai): internal 24→16kHz mini-downsample in VAD wrapper (cheap 3:2 decimation)
- [ ] Remove `WhisperEngine::has_speech()` and `VAD_ENERGY_THRESHOLD`
- [ ] Preload VAD in parallel with model loading

**Alternatives considered**:
- `webrtc-vad` 0.1.0 — GMM-based, faster (<1ms) but significantly less accurate than neural
- `silero-vad-rust` 6.2.0 — supports v4+v5 but less maintained
- Custom energy VAD — current approach, too primitive

#### Step 2 — Text Post-Processing Pipeline

**Problem**: Raw Whisper/Kyutai output contains filler words ("uh", "um", "euh"), stutters ("wh wh what"), and no cross-engine text cleanup.

**Solution**: Composable text filter chain applied after engine-specific `normalize_text()`.

**Architecture**:
- [ ] New file: `src-tauri/src/pipeline/text_filters.rs`
- [ ] Trait: `TextFilter { fn apply(&mut self, text: &str) -> String; fn reset(&mut self); }`
- [ ] Filters built as `Vec<Box<dyn TextFilter>>` per-session from user settings
- [ ] Applied in `emit_normalized()` after `engine.normalize_text()`

**Filters**:
1. `FillerWordFilter` — regex removal of "uh", "um", "ah", "hmm", "euh", "hein", etc. (~50 LOC)
2. `StutterCollapseFilter` — collapse 3+ consecutive repetitions of ANY word (~40 LOC)
3. `WhitespaceNormalizationFilter` — wraps existing `collapse_whitespace()`

No new crate needed — pure Rust regex + string processing.

#### Step 3 — Custom Word Dictionary with Fuzzy Matching

**Problem**: STT misrecognizes proper nouns, technical terms, company names.

**Solution**: User-maintained dictionary with Levenshtein + Soundex correction.

**Crates**:
- `strsim` 0.11 (rapidfuzz team) — Levenshtein, Jaro-Winkler, Damerau-Levenshtein
- Hand-rolled Soundex (~40 LOC) — no good maintained Rust crate exists

**Architecture**:
- [ ] New SQLite table: `custom_dictionary(id, word, phonetic_code, category, created_at)`
- [ ] New file: `src-tauri/src/db/dictionary.rs` — CRUD operations
- [ ] New file: `src-tauri/src/commands/dictionary.rs` — Tauri commands
- [ ] `CustomWordFilter` in text_filters.rs — loads dictionary at session start, applies per-segment
- [ ] Matching: normalized Levenshtein distance < 0.18 AND/OR Soundex match → replace
- [ ] Frontend: DictionarySettingsSection.svelte for word list management (add/remove/import)

#### Step 4 — Settings Integration

- [ ] New settings: `enable_vad` (default: true), `enable_filler_removal` (default: true), `enable_stutter_collapse` (default: false), `enable_custom_dictionary` (default: true)
- [ ] Frontend: Replace "coming soon" pills in AudioSettingsSection with actual toggles
- [ ] All filters configurable per-user, stutter collapse off by default (some users want verbatim)

### Phase 7B: Pipeline Features

#### Step 5 — Pipeline Filter Architecture (Refactor)

Formalize audio + text processing into composable filter chains.

**Audio filters** — trait: `AudioFilter { fn process(&mut self, samples: &[f32], sample_rate: u32) -> Vec<f32>; fn reset(&mut self); }`
- [ ] `VadGateFilter` — wraps SmoothedVad from Step 1
- [ ] `NoiseGateFilter` — simple amplitude gate (future)
- [ ] `GainFilter` — wraps existing mic_gain logic

**Text filters** — already done in Step 2, formalize into same filter chain architecture.

Benefits ALL engines (Whisper, Kyutai, future Parakeet).

#### Step 6 — Model Unload Timeout

**Problem**: STT models consume 1.6–5.6GB GPU memory even when idle.

**Solution**: Configurable auto-unload after idle period.
- [ ] Options: Never (default), Immediately, 2/5/10/15 min, 1 hour
- [ ] Implementation: `cmd_rx.recv_timeout()` in inference thread idle loop
- [ ] On timeout: `engine.unload_model()`, emit state change event
- [ ] On next Start: detect unloaded, trigger reload

#### Step 7 — Save Audio Recording (WAV)

**Problem**: Can't re-transcribe with different model, audio lost after session.

**Solution**: Save audio to WAV alongside transcript.
- [ ] Save BEFORE transcription starts (data safety)
- [ ] Crate: `hound` 3.5 (already in Cargo.toml)
- [ ] Format: 16-bit PCM mono at engine target rate
- [ ] Storage: `{app_data_dir}/recordings/{meeting_id}.wav`
- [ ] ~1.9 MB/min at 16kHz → ~115 MB/hour
- [ ] Setting: `save_audio_recordings: bool` (default: false)
- [ ] Add `audio_file_path` column to meetings table

#### Step 8 — SHA256 Model Verification

**Problem**: Corrupt partial downloads cause infinite retry loops.

**Solution**: SHA256 checksum verification after download.
- [ ] Crate: `sha2` (likely already transitive dep)
- [ ] Hardcode expected hashes for known models
- [ ] On mismatch: delete partial file, emit error event, retry from scratch
- [ ] Custom models: skip verification

#### Step 9 — Clamshell Mode (macOS)

**Problem**: Must manually re-select mic when closing/opening laptop lid.

**Solution**: Auto-switch to configured external mic on clamshell.
- [ ] Detect device changes via cpal device enumeration (poll or CoreAudio notification)
- [ ] Setting: `clamshell_mic_device: Option<String>`
- [ ] When built-in mic disappears → switch to clamshell device
- [ ] When built-in mic reappears → switch back

#### Step 10 — Always-On Microphone

**Problem**: ~50–200ms stream startup latency on recording start.

**Solution**: Keep cpal stream open between recordings.
- [ ] Stream stays in `play()` state, audio callback checks active session
- [ ] When no session: still compute RMS for waveform, don't send to pipeline
- [ ] On Start: just set session_id, instant audio delivery
- [ ] Setting: `always_on_mic: bool` (default: false)

#### Step 11 — History Pagination

**Problem**: Large history loads all entries at once.

**Solution**: Paginated queries.
- [ ] `get_meetings(limit, offset) → PaginatedResult { entries, has_more }`
- [ ] Same for dictation history
- [ ] Frontend: infinite scroll or "Load more" button

#### Step 12 — GPU Enumeration (Whisper)

**Problem**: No way to select specific GPU for Whisper inference.

**Solution**: List available GPUs with VRAM info.
- [ ] Expose via whisper-rs/whisper.cpp GPU enumeration
- [ ] Setting: `whisper_gpu_device: i32` (Auto = -1)
- [ ] Frontend: GPU dropdown in engine settings with VRAM display
- [ ] Cache GPU list at startup (OnceLock)

#### Implementation Order

1. Step 1 (Silero VAD) — Highest impact on quality
2. Step 2 (Text filters) — Second highest impact
3. Step 3 (Custom dictionary) — Builds on Step 2
4. Step 4 (Settings) — Wire Steps 1–3 to UI
5. Step 7 (WAV recording) — Data safety, enables re-transcription
6. Step 8 (SHA256 verification) — Download reliability
7. Step 6 (Model unload) — Memory management
8. Step 11 (History pagination) — Performance
9. Step 10 (Always-on mic) — Latency improvement
10. Step 5 (Filter architecture refactor) — Formalize pipeline
11. Step 9 (Clamshell) — macOS-specific, CoreAudio FFI complexity
12. Step 12 (GPU enumeration) — Nice-to-have for multi-GPU systems

#### Engine Impact Matrix

| Feature | Kyutai | Whisper | Parakeet (future) |
|---------|--------|---------|-------------------|
| Silero VAD | Drop non-speech | Replace energy VAD | Shared |
| Filler removal | High impact | Moderate | Shared |
| Stutter collapse | High (streaming) | Low (batch) | Shared |
| Custom dictionary | All | All | All |
| Model unload | ~2.4–5.6GB freed | ~1.6GB freed | Shared |
| WAV recording | All | All | All |

#### Library Summary

| Component | Recommended | Version | Rationale |
|-----------|------------|---------|-----------|
| VAD | `voice-activity-detector` | 0.2.1 | Best maintained, bundles Silero v5 ONNX |
| ONNX Runtime | `ort` (transitive) | 2.0.0-rc.10+ | Required by VAD crate |
| String similarity | `strsim` | 0.11 | Well-maintained (rapidfuzz team) |
| Phonetic matching | Hand-rolled Soundex | ~40 LOC | No good Rust crate exists |
| Text cleanup | Custom module | ~120 LOC | No Rust crate for STT text cleanup |
| WAV recording | `hound` | 3.5 | Already in Cargo.toml |
| SHA256 | `sha2` | latest | Standard, likely already transitive |

**NOT recommended**: `transcribe-rs` (batch-only, no streaming), `natural` (poorly maintained), `ttaw` (low adoption), `webrtc-vad` (less accurate than Silero neural VAD)

---

## 9. Risks & Mitigations

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| Candle Metal perf on M4 Pro insufficient for real-time 1B model | Critical | Medium | **Validate in Phase 2 ASAP.** Fallback: Whisper via whisper-rs (proven). Candle Metal is less optimized than whisper.cpp Metal. |
| Kyutai stt-rs is demo-quality, not production-ready | High | Medium | Budget extra time to harden. Isolate in engine trait so replacement is cheap. |
| ~~Native CoreAudio loopback / system-audio capture on macOS remains fragile~~ | ~~High~~ | ~~Low~~ | ✅ **Resolved on 2026-06-12**: native Core Audio process tap + programmatic aggregate device (macOS 14.4+), with mic-only graceful degradation. Fallback if needed: ScreenCaptureKit. |
| sonora (pure-Rust WebRTC AEC3) is v0.1 and may have quality gaps | Medium | Medium | Isolated behind `audio/aec.rs` wrapper; drop-in fallback is the C++-backed `webrtc-audio-processing` crate. |
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
        capture.rs         # cpal microphone capture + meeting mixer tick loop
        system_tap.rs      # Core Audio process tap (system audio, macOS 14.4+)
        mixer.rs           # Two-source meeting mixer (mic + system audio)
        aec.rs             # Echo cancellation wrapper (sonora / WebRTC AEC3)
        output_route.rs    # Default-output route detection (speakers vs headphones)
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
