//! Engine actor: a single thread that owns the transcription engine outright.
//!
//! Engine creation, model load/unload, session inference, swap, and drop all
//! happen on this one thread, driven by `EngineCommand`s. This keeps every
//! Metal/ObjC and native-library teardown on the same thread and removes the
//! shared `Arc<Mutex<Box<dyn TranscriptionEngine>>>` that previously smeared
//! the engine lifecycle across command and inference threads.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use tauri::Manager;
use tauri_specta::Event;
use tracing::{debug, error, info, warn};

use crate::app_events::{PipelineError, PipelineErrorScope};
use crate::audio::{AudioChunk, AudioMessage};
use crate::engine::{
    AudioInputRequirements, Speaker, TranscriptionEngine, TranscriptionProfile,
    TranscriptionSegment,
};
use crate::filter::{
    AudioFilterChain, DictionaryEntry, PipelineConfig, TextFilterChain, build_audio_filters,
    build_text_filters,
};
use crate::platform::with_autorelease_pool;

use super::SegmentCallback;
use super::health::SessionHealth;

/// Creates engines ON the actor thread. Production uses `crate::engine::create_engine`;
/// tests inject factories that produce mock engines.
pub type EngineFactory =
    Box<dyn Fn(&TranscriptionProfile) -> Result<Box<dyn TranscriptionEngine>, String> + Send>;

/// Audio/gain requirements of the loaded engine, returned to the command layer
/// so it can configure audio capture.
#[derive(Debug, Clone)]
pub struct EngineInfo {
    pub audio: AudioInputRequirements,
    pub mic_gain: f32,
}

/// Per-session statistics returned when a session stops.
#[derive(Debug, Clone, Default)]
pub struct SessionSummary {
    pub frames_processed: u64,
    pub skipped_chunks: u64,
    /// Chunks the capture callback dropped because the channel was full.
    pub dropped_chunks: u64,
}

/// Snapshot of settings the actor needs to build filter chains for a session.
pub struct SessionConfig {
    pub pipeline_config: PipelineConfig,
    pub dictionary_entries: Vec<DictionaryEntry>,
    /// Diarized meeting: run a second engine so the mic (Me) and system audio
    /// (Them) legs are transcribed separately and segments are speaker-tagged.
    pub diarize: bool,
}

pub enum EngineCommand {
    /// Give the actor an AppHandle so it can emit health/error events and
    /// fail the state machine on mid-session fatalities. Sent once in setup.
    AttachApp(tauri::AppHandle),
    LoadModel {
        profile: TranscriptionProfile,
        model_dir: PathBuf,
        reply: Sender<Result<EngineInfo, String>>,
    },
    UnloadModel {
        reply: Sender<Result<(), String>>,
    },
    StartSession {
        session_id: u64,
        config: SessionConfig,
        on_segment: SegmentCallback,
        reply: Sender<Result<EngineInfo, String>>,
    },
    StopSession {
        reply: Sender<Result<SessionSummary, String>>,
    },
    DebugTranscribe {
        samples: Vec<f32>,
        reply: Sender<Result<Vec<TranscriptionSegment>, String>>,
    },
    Shutdown,
}

/// Handle owned by `AppState`. All methods are synchronous command/reply
/// round-trips with the actor thread.
pub struct EngineActorHandle {
    cmd_tx: Sender<EngineCommand>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl EngineActorHandle {
    /// Spawn the actor thread. It owns the audio receiver and (eventually) the engine.
    /// `dropped_counter` is shared with the audio capture callback for health reporting.
    pub fn spawn(
        audio_rx: Receiver<AudioMessage>,
        dropped_counter: Arc<AtomicU64>,
        factory: EngineFactory,
    ) -> Result<Self, String> {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<EngineCommand>();

        let handle = std::thread::Builder::new()
            .name("engine-actor".into())
            .spawn(move || {
                EngineActor {
                    cmd_rx,
                    audio_rx,
                    factory,
                    engine: None,
                    app: None,
                    dropped_counter,
                }
                .run();
            })
            .map_err(|e| format!("Failed to spawn engine actor thread: {e}"))?;

        Ok(Self {
            cmd_tx,
            handle: Mutex::new(Some(handle)),
        })
    }

    fn request<T>(
        &self,
        make: impl FnOnce(Sender<Result<T, String>>) -> EngineCommand,
        timeout: Option<Duration>,
    ) -> Result<T, String> {
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        self.cmd_tx
            .send(make(reply_tx))
            .map_err(|_| "Engine actor disconnected".to_string())?;
        match timeout {
            Some(t) => reply_rx
                .recv_timeout(t)
                .map_err(|_| "Engine actor reply timeout".to_string())?,
            None => reply_rx
                .recv()
                .map_err(|_| "Engine actor disconnected".to_string())?,
        }
    }

    /// Fire-and-forget: hand the actor an AppHandle for event emission.
    pub fn attach_app(&self, app: tauri::AppHandle) {
        let _ = self.cmd_tx.send(EngineCommand::AttachApp(app));
    }

    /// Load (or swap to) a model. Blocks for the duration of the load —
    /// model loads legitimately take tens of seconds.
    pub fn load_model(
        &self,
        profile: TranscriptionProfile,
        model_dir: PathBuf,
    ) -> Result<EngineInfo, String> {
        self.request(
            |reply| EngineCommand::LoadModel {
                profile,
                model_dir,
                reply,
            },
            None,
        )
    }

    pub fn unload_model(&self) -> Result<(), String> {
        self.request(|reply| EngineCommand::UnloadModel { reply }, None)
    }

    /// Start a transcription session. Replies once the engine state is reset
    /// and filter chains are built, BEFORE audio capture should begin.
    pub fn start_session(
        &self,
        session_id: u64,
        config: SessionConfig,
        on_segment: SegmentCallback,
    ) -> Result<EngineInfo, String> {
        self.request(
            |reply| EngineCommand::StartSession {
                session_id,
                config,
                on_segment,
                reply,
            },
            None,
        )
    }

    /// Stop the active session: the actor drains buffered audio, flushes the
    /// engine, and replies with session stats. The timeout is a last-resort
    /// safety net — callers must complete their state transition even on Err.
    pub fn stop_session(&self, timeout: Duration) -> Result<SessionSummary, String> {
        self.request(|reply| EngineCommand::StopSession { reply }, Some(timeout))
    }

    /// Feed raw samples through reset + transcribe, for diagnostics.
    pub fn debug_transcribe(&self, samples: Vec<f32>) -> Result<Vec<TranscriptionSegment>, String> {
        self.request(
            |reply| EngineCommand::DebugTranscribe { samples, reply },
            None,
        )
    }

    /// Shut down the actor thread: unloads and drops the engine on the actor
    /// thread, then joins. Idempotent.
    pub fn shutdown(&self) -> Result<(), String> {
        let handle = {
            let mut guard = self
                .handle
                .lock()
                .map_err(|e| format!("Lock poisoned: {e}"))?;
            guard.take()
        };
        if let Some(handle) = handle {
            let _ = self.cmd_tx.send(EngineCommand::Shutdown);
            handle
                .join()
                .map_err(|_| "Engine actor thread panicked during shutdown".to_string())?;
        }
        Ok(())
    }
}

impl Drop for EngineActorHandle {
    fn drop(&mut self) {
        if let Err(e) = self.shutdown() {
            warn!("Engine actor drop failed: {e}");
        }
    }
}

/// The actor itself — only ever touched by its own thread.
struct EngineActor {
    cmd_rx: Receiver<EngineCommand>,
    audio_rx: Receiver<AudioMessage>,
    factory: EngineFactory,
    engine: Option<Box<dyn TranscriptionEngine>>,
    app: Option<tauri::AppHandle>,
    dropped_counter: Arc<AtomicU64>,
}

/// Outcome of an active session loop.
enum SessionEnd {
    /// Session stopped normally; reply sender for the summary.
    Stopped(Sender<Result<SessionSummary, String>>, SessionSummary),
    /// Session never started — error already reported on the setup reply.
    SetupFailed,
    /// Session aborted mid-recording (e.g. repeated engine failures).
    Aborted(String),
    /// Shutdown requested mid-session.
    Shutdown,
    /// Audio channel disconnected — nothing more can ever be transcribed.
    AudioGone,
}

impl EngineActor {
    fn run(mut self) {
        info!("Engine actor started");
        let mut session_count: u32 = 0;

        loop {
            let Ok(cmd) = self.cmd_rx.recv() else {
                warn!("Engine actor command channel disconnected, exiting");
                break;
            };

            match cmd {
                EngineCommand::AttachApp(app) => {
                    self.app = Some(app);
                }
                EngineCommand::LoadModel {
                    profile,
                    model_dir,
                    reply,
                } => {
                    let result = with_autorelease_pool(|| self.handle_load(&profile, &model_dir));
                    let _ = reply.send(result);
                }
                EngineCommand::UnloadModel { reply } => {
                    with_autorelease_pool(|| self.drop_engine());
                    let _ = reply.send(Ok(()));
                }
                EngineCommand::StartSession {
                    session_id,
                    config,
                    on_segment,
                    reply,
                } => {
                    session_count += 1;
                    info!(
                        "Inference session {session_count} starting (audio session {session_id})"
                    );
                    let end = with_autorelease_pool(|| {
                        self.handle_session(session_id, config, on_segment, reply)
                    });
                    match end {
                        SessionEnd::Stopped(done, summary) => {
                            let _ = done.send(Ok(summary));
                            info!("Inference session {session_count} ended");
                        }
                        SessionEnd::SetupFailed => {
                            warn!("Inference session {session_count} failed during setup");
                        }
                        SessionEnd::Aborted(message) => {
                            error!("Inference session {session_count} aborted: {message}");
                            self.handle_session_abort(message);
                        }
                        SessionEnd::Shutdown => break,
                        SessionEnd::AudioGone => {
                            // The audio thread vanished mid-session. Rather than
                            // kill the actor (which would silently swallow every
                            // future command), salvage the meeting and surface a
                            // recoverable error so the user can retry — same path
                            // as a session abort. The actor stays alive.
                            error!("Audio channel disconnected mid-session; recovering");
                            self.handle_session_abort(
                                "Audio capture stopped unexpectedly".to_string(),
                            );
                        }
                    }
                }
                EngineCommand::StopSession { reply } => {
                    // Already idle — stop is idempotent.
                    let _ = reply.send(Ok(SessionSummary::default()));
                }
                EngineCommand::DebugTranscribe { samples, reply } => {
                    let result = with_autorelease_pool(|| self.handle_debug_transcribe(&samples));
                    let _ = reply.send(result);
                }
                EngineCommand::Shutdown => break,
            }
        }

        // Unload + drop the engine on this thread before exiting. whisper.cpp's
        // Metal residency sets and Candle's Metal objects must be released here,
        // not from a static destructor at process exit.
        with_autorelease_pool(|| self.drop_engine());
        info!("Engine actor shut down cleanly");
    }

    /// A session died mid-recording: surface the error to the frontend, then
    /// let `AppState` stop audio capture, salvage any in-progress meeting,
    /// and fail the state machine so the UI leaves the recording state.
    /// Without this, the old pipeline died silently and the app looked like
    /// it "just stopped transcribing". The pipeline layer only owns emitting
    /// the event here; the rest is app-level cleanup that doesn't belong on
    /// this side of the actor/command boundary.
    fn handle_session_abort(&mut self, message: String) {
        if let Some(app) = &self.app {
            let _ = PipelineError {
                scope: PipelineErrorScope::Session,
                message: message.clone(),
            }
            .emit(app);

            app.state::<crate::state::AppState>()
                .abort_active_session(message);
        }
        // Drain whatever audio is still queued (including the EndOfStream the
        // audio thread sends on Stop) so nothing stale leaks into the next session.
        self.drain_audio_queue();
    }

    fn drop_engine(&mut self) {
        if let Some(mut engine) = self.engine.take() {
            if let Err(e) = engine.unload_model() {
                warn!("Engine unload failed: {e}");
            }
            drop(engine);
        }
    }

    fn handle_load(
        &mut self,
        profile: &TranscriptionProfile,
        model_dir: &std::path::Path,
    ) -> Result<EngineInfo, String> {
        // Swap = unload + drop old, then create + load new — all sequential,
        // all on this thread.
        self.drop_engine();

        let mut engine = (self.factory)(profile)?;
        engine.load_model(model_dir).map_err(|e| e.to_string())?;
        let info = EngineInfo {
            audio: engine.audio_requirements(),
            mic_gain: engine.mic_gain(),
        };
        self.engine = Some(engine);
        Ok(info)
    }

    fn handle_debug_transcribe(
        &mut self,
        samples: &[f32],
    ) -> Result<Vec<TranscriptionSegment>, String> {
        let engine = self.engine.as_mut().ok_or("No model loaded")?;
        engine.reset_state().map_err(|e| e.to_string())?;
        engine.transcribe(samples, None).map_err(|e| e.to_string())
    }

    /// Prepare and run one transcription session. Replies on `reply` once the
    /// engine is reset and filters are built (success), or with the error.
    fn handle_session(
        &mut self,
        session_id: u64,
        config: SessionConfig,
        on_segment: SegmentCallback,
        reply: Sender<Result<EngineInfo, String>>,
    ) -> SessionEnd {
        // Clear stale audio left over from previous sessions before resetting.
        let drained = self.drain_audio_queue();
        if drained > 0 && crate::debug::transcription_debug_enabled() {
            debug!(drained, "Cleared stale audio chunks before session start");
        }

        let Some(engine) = self.engine.as_mut() else {
            let _ = reply.send(Err("No model loaded".into()));
            return SessionEnd::SetupFailed;
        };

        // Diarize only if requested AND the engine can run two batched lanes.
        // Otherwise the meeting falls back to a single mixed stream.
        let diarize = config.diarize && engine.supports_diarization();
        engine.set_diarization(diarize);

        // Reset rebuilds streaming state with the right batch size (2 when
        // diarized, 1 otherwise).
        if let Err(e) = engine.reset_state() {
            let _ = reply.send(Err(format!("State reset: {e}")));
            return SessionEnd::SetupFailed;
        }

        let info = EngineInfo {
            audio: engine.audio_requirements(),
            mic_gain: engine.mic_gain(),
        };
        let sample_rate = info.audio.sample_rate_hz;

        // Filter chains are built on this thread so ONNX Runtime (Silero VAD)
        // initialization shares the thread with engine Metal work.
        let text_filters = build_text_filters(&config.pipeline_config, config.dictionary_entries);

        let mut health = SessionHealth::start(session_id, Arc::clone(&self.dropped_counter));
        let engine = self.engine.as_mut().expect("engine present: checked above");

        // No Silero VAD in diarized mode: the two lanes must step together
        // every frame to stay aligned in the batch, so we can't drop a silent
        // frame from one side. Kyutai's own semantic VAD handles pauses.
        let mut mode: Box<dyn SessionMode> = if diarize {
            Box::new(DiarizedMode::new())
        } else {
            Box::new(SingleMode::new(build_audio_filters(
                &config.pipeline_config,
                sample_rate,
            )))
        };

        // Caller may now start audio capture.
        let _ = reply.send(Ok(info));

        run_session_loop(
            &self.cmd_rx,
            &self.audio_rx,
            engine.as_mut(),
            &on_segment,
            session_id,
            &text_filters,
            &mut health,
            self.app.as_ref(),
            mode.as_mut(),
        )
    }

    fn drain_audio_queue(&self) -> usize {
        let mut drained = 0usize;
        while self.audio_rx.try_recv().is_ok() {
            drained += 1;
        }
        drained
    }
}

/// How long stop waits for the audio thread's EndOfStream marker before
/// draining anyway. Only reached if the audio thread died mid-session.
const EOS_WAIT: Duration = Duration::from_secs(5);

/// Abort the session after this many consecutive transcribe failures
/// (~2 seconds of audio at Kyutai's 80ms frames).
const MAX_CONSECUTIVE_FRAME_ERRORS: u32 = 25;

/// A session's audio buffering and engine-stepping strategy. The generic loop
/// (`run_session_loop`) owns everything session-lifecycle-shaped (commands,
/// stop sequencing, health, heartbeat, the error-streak abort); a mode only
/// knows how to buffer chunks and turn a buffered frame into engine calls.
trait SessionMode {
    /// Buffer a chunk's samples (single buffer, or routed by `chunk.speaker`).
    fn ingest(&mut self, chunk: AudioChunk);

    /// True while a full engine frame can be drained from the buffer(s).
    fn frame_ready(&self, chunk_size: usize) -> bool;

    /// Drain one frame and run the engine on it. `Ok(None)` means the frame
    /// was VAD-skipped (single mode only): frame/health counters still
    /// advance, but the consecutive-error streak does not move. The caller
    /// wraps this call in `catch_unwind`.
    fn step(
        &mut self,
        engine: &mut dyn TranscriptionEngine,
        chunk_size: usize,
    ) -> Result<Option<Vec<TranscriptionSegment>>, String>;

    /// Drain and transcribe one full frame unconditionally (no VAD gating):
    /// used at session end, where every buffered frame must still reach the
    /// engine even if VAD would otherwise have gated it.
    fn finish_frame(
        &mut self,
        engine: &mut dyn TranscriptionEngine,
        chunk_size: usize,
    ) -> Result<Vec<TranscriptionSegment>, String>;

    /// Feed whatever partial tail(s) remain (less than a full frame) through
    /// the engine at session end.
    fn finish_tail(
        &mut self,
        engine: &mut dyn TranscriptionEngine,
    ) -> Result<Vec<TranscriptionSegment>, String>;

    /// Emit the periodic heartbeat log line with mode-specific fields.
    fn log_heartbeat(
        &self,
        session_id: u64,
        frames_processed: u64,
        segments_emitted: u64,
        audio_backlog: usize,
    );
}

/// Single mixed-stream session: one buffer gated by the configured audio
/// filter chain (Silero VAD when enabled).
struct SingleMode {
    buffer: Vec<f32>,
    audio_filters: AudioFilterChain,
    vad_skipped: u64,
}

impl SingleMode {
    fn new(audio_filters: AudioFilterChain) -> Self {
        Self {
            buffer: Vec::new(),
            audio_filters,
            vad_skipped: 0,
        }
    }
}

impl SessionMode for SingleMode {
    fn ingest(&mut self, chunk: AudioChunk) {
        self.buffer.extend_from_slice(&chunk.samples);
    }

    fn frame_ready(&self, chunk_size: usize) -> bool {
        self.buffer.len() >= chunk_size
    }

    fn step(
        &mut self,
        engine: &mut dyn TranscriptionEngine,
        chunk_size: usize,
    ) -> Result<Option<Vec<TranscriptionSegment>>, String> {
        let frame: Vec<f32> = self.buffer.drain(..chunk_size).collect();
        if !self.audio_filters.process(&frame) {
            self.vad_skipped += 1;
            return Ok(None); // VAD says no speech — skip this frame
        }
        engine
            .transcribe(&frame, None)
            .map(Some)
            .map_err(|e| e.to_string())
    }

    fn finish_frame(
        &mut self,
        engine: &mut dyn TranscriptionEngine,
        chunk_size: usize,
    ) -> Result<Vec<TranscriptionSegment>, String> {
        let frame: Vec<f32> = self.buffer.drain(..chunk_size).collect();
        catch_engine(|| engine.transcribe(&frame, None))
    }

    fn finish_tail(
        &mut self,
        engine: &mut dyn TranscriptionEngine,
    ) -> Result<Vec<TranscriptionSegment>, String> {
        if self.buffer.is_empty() {
            return Ok(Vec::new());
        }
        let result = catch_engine(|| engine.transcribe(&self.buffer, None));
        self.buffer.clear();
        result
    }

    fn log_heartbeat(
        &self,
        session_id: u64,
        frames_processed: u64,
        segments_emitted: u64,
        audio_backlog: usize,
    ) {
        info!(
            session_id,
            transcribed_frames = frames_processed,
            vad_skipped = self.vad_skipped,
            segments_emitted,
            audio_backlog,
            "Session heartbeat"
        );
    }
}

/// Diarized session: mic (Me) and system audio (Them) buffered separately and
/// stepped together into one batched `transcribe_dual` call, so the two lanes
/// stay frame-aligned. No Silero VAD: a silent frame can't be dropped from
/// just one side without breaking that alignment (Kyutai's own semantic VAD
/// handles pauses).
struct DiarizedMode {
    me_buf: Vec<f32>,
    them_buf: Vec<f32>,
}

impl DiarizedMode {
    fn new() -> Self {
        Self {
            me_buf: Vec::new(),
            them_buf: Vec::new(),
        }
    }

    /// Untagged audio (shouldn't happen in diarized mode) goes to the mic.
    fn buf_for(&mut self, speaker: Option<Speaker>) -> &mut Vec<f32> {
        match speaker {
            Some(Speaker::Them) => &mut self.them_buf,
            _ => &mut self.me_buf,
        }
    }
}

impl SessionMode for DiarizedMode {
    fn ingest(&mut self, chunk: AudioChunk) {
        self.buf_for(chunk.speaker)
            .extend_from_slice(&chunk.samples);
    }

    fn frame_ready(&self, chunk_size: usize) -> bool {
        self.me_buf.len() >= chunk_size && self.them_buf.len() >= chunk_size
    }

    fn step(
        &mut self,
        engine: &mut dyn TranscriptionEngine,
        chunk_size: usize,
    ) -> Result<Option<Vec<TranscriptionSegment>>, String> {
        let me_frame: Vec<f32> = self.me_buf.drain(..chunk_size).collect();
        let them_frame: Vec<f32> = self.them_buf.drain(..chunk_size).collect();
        engine
            .transcribe_dual(&me_frame, &them_frame)
            .map(Some)
            .map_err(|e| e.to_string())
    }

    fn finish_frame(
        &mut self,
        engine: &mut dyn TranscriptionEngine,
        chunk_size: usize,
    ) -> Result<Vec<TranscriptionSegment>, String> {
        let me_frame: Vec<f32> = self.me_buf.drain(..chunk_size).collect();
        let them_frame: Vec<f32> = self.them_buf.drain(..chunk_size).collect();
        catch_engine(|| engine.transcribe_dual(&me_frame, &them_frame))
    }

    fn finish_tail(
        &mut self,
        engine: &mut dyn TranscriptionEngine,
    ) -> Result<Vec<TranscriptionSegment>, String> {
        if self.me_buf.is_empty() && self.them_buf.is_empty() {
            return Ok(Vec::new());
        }
        // transcribe_dual zero-pads the shorter lane internally.
        let me_tail = std::mem::take(&mut self.me_buf);
        let them_tail = std::mem::take(&mut self.them_buf);
        catch_engine(|| engine.transcribe_dual(&me_tail, &them_tail))
    }

    fn log_heartbeat(
        &self,
        session_id: u64,
        frames_processed: u64,
        segments_emitted: u64,
        audio_backlog: usize,
    ) {
        info!(
            session_id,
            transcribed_frames = frames_processed,
            segments_emitted,
            audio_backlog,
            "Diarized session heartbeat"
        );
    }
}

/// Process audio frames while the session is active. Owns command handling,
/// health/heartbeat, the EOS-wait stop sequencing, and the consecutive-error
/// abort logic; `mode` decides how audio is buffered and stepped through the
/// engine (single stream + VAD, or paired diarized lanes).
#[allow(clippy::too_many_arguments)]
fn run_session_loop(
    cmd_rx: &Receiver<EngineCommand>,
    audio_rx: &Receiver<AudioMessage>,
    engine: &mut dyn TranscriptionEngine,
    on_segment: &SegmentCallback,
    session_id: u64,
    text_filters: &TextFilterChain,
    health: &mut SessionHealth,
    app: Option<&tauri::AppHandle>,
    mode: &mut dyn SessionMode,
) -> SessionEnd {
    let chunk_size = engine.audio_requirements().chunk_size_samples as usize;
    let mut summary = SessionSummary::default();
    let mut eos_received = false;
    let mut consecutive_errors: u32 = 0;
    // Stop request waiting for this session's EndOfStream marker.
    let mut pending_stop: Option<(Sender<Result<SessionSummary, String>>, Instant)> = None;

    // Always-on diagnostics: a session that silently stops transcribing (engine
    // emits nothing, or VAD gates everything) looks identical to "still
    // recording" from the UI. This heartbeat makes the failure mode legible in
    // the log without the verbose debug setting.
    let mut segments_emitted: u64 = 0;
    let mut last_heartbeat = Instant::now();
    const HEARTBEAT: Duration = Duration::from_secs(30);

    loop {
        // Periodic health snapshot to the frontend
        if let Some(snapshot) = health.tick(audio_rx.len())
            && let Some(app) = app
        {
            let _ = snapshot.emit(app);
        }

        if last_heartbeat.elapsed() >= HEARTBEAT {
            last_heartbeat = Instant::now();
            mode.log_heartbeat(
                session_id,
                summary.frames_processed,
                segments_emitted,
                audio_rx.len(),
            );
        }

        // Check for commands (non-blocking)
        match cmd_rx.try_recv() {
            Ok(EngineCommand::AttachApp(_)) => {}
            Ok(EngineCommand::StopSession { reply }) => {
                if crate::debug::transcription_debug_enabled() {
                    debug!("Stopping ({} frames processed)", summary.frames_processed);
                }
                if eos_received {
                    summary.dropped_chunks = health.dropped_chunks();
                    finish_session(
                        audio_rx,
                        engine,
                        on_segment,
                        session_id,
                        text_filters,
                        mode,
                        &mut summary,
                    );
                    return SessionEnd::Stopped(reply, summary);
                }
                // All audio up to the stream drop is still in flight —
                // keep consuming until EndOfStream arrives.
                pending_stop = Some((reply, Instant::now()));
            }
            Ok(EngineCommand::Shutdown) => return SessionEnd::Shutdown,
            Ok(EngineCommand::LoadModel { reply, .. }) => {
                let _ = reply.send(Err("Recording session active".into()));
            }
            Ok(EngineCommand::UnloadModel { reply }) => {
                let _ = reply.send(Err("Recording session active".into()));
            }
            Ok(EngineCommand::StartSession { reply, .. }) => {
                let _ = reply.send(Err("Recording session active".into()));
            }
            Ok(EngineCommand::DebugTranscribe { reply, .. }) => {
                let _ = reply.send(Err("Recording session active".into()));
            }
            Err(_) => {}
        }

        // Safety net: if stop was requested but EndOfStream never arrives
        // (audio thread died), drain what we have and finish anyway.
        if let Some((_, requested_at)) = &pending_stop
            && requested_at.elapsed() > EOS_WAIT
        {
            warn!("End-of-stream marker never arrived; finishing session anyway");
            let (reply, _) = pending_stop.take().expect("pending_stop checked above");
            summary.dropped_chunks = health.dropped_chunks();
            finish_session(
                audio_rx,
                engine,
                on_segment,
                session_id,
                text_filters,
                mode,
                &mut summary,
            );
            return SessionEnd::Stopped(reply, summary);
        }

        // Read audio with short timeout so command checks stay responsive
        match audio_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(AudioMessage::EndOfStream { session_id: eos_id }) => {
                if eos_id != session_id {
                    continue; // stale marker from a previous session
                }
                eos_received = true;
                if let Some((reply, _)) = pending_stop.take() {
                    summary.dropped_chunks = health.dropped_chunks();
                    finish_session(
                        audio_rx,
                        engine,
                        on_segment,
                        session_id,
                        text_filters,
                        mode,
                        &mut summary,
                    );
                    return SessionEnd::Stopped(reply, summary);
                }
            }
            Ok(AudioMessage::Chunk(chunk)) => {
                if chunk.session_id != session_id {
                    summary.skipped_chunks += 1;
                    if crate::debug::transcription_debug_enabled()
                        && (summary.skipped_chunks <= 5
                            || summary.skipped_chunks.is_multiple_of(25))
                    {
                        debug!(
                            "Ignoring stale audio chunk from session {} while expecting {}",
                            chunk.session_id, session_id
                        );
                    }
                    continue;
                }

                health.note_chunk(chunk.captured_at);
                mode.ingest(chunk);

                // Process complete engine-sized frames
                while mode.frame_ready(chunk_size) {
                    // A panic in the engine (candle/whisper/Metal) must not
                    // abort the whole app: catch it and treat it like a frame
                    // error so the streak logic below can recover or abort the
                    // session cleanly.
                    let step_result: Result<Option<Vec<TranscriptionSegment>>, String> =
                        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            mode.step(engine, chunk_size)
                        })) {
                            Ok(inner) => inner,
                            Err(_) => Err("engine panicked during transcribe".to_string()),
                        };
                    match step_result {
                        Ok(None) => {
                            // VAD-skipped frame (single mode only).
                            summary.frames_processed += 1;
                            health.note_frame();
                            continue;
                        }
                        Ok(Some(segments)) => {
                            consecutive_errors = 0;
                            for seg in segments {
                                if crate::debug::transcription_debug_enabled() {
                                    debug!("Segment: {:?} final={}", seg.text, seg.is_final);
                                }
                                segments_emitted += 1;
                                emit_filtered(engine, text_filters, seg, on_segment);
                            }
                        }
                        Err(e) => {
                            // A lone failure is skipped (and surfaced); a streak
                            // means the engine is broken — abort instead of
                            // silently eating audio for the rest of the session.
                            consecutive_errors += 1;
                            error!("Transcribe error (frame skipped): {e}");
                            if consecutive_errors == 1
                                && let Some(app) = app
                            {
                                let _ = PipelineError {
                                    scope: PipelineErrorScope::Frame,
                                    message: e.clone(),
                                }
                                .emit(app);
                            }
                            if consecutive_errors >= MAX_CONSECUTIVE_FRAME_ERRORS {
                                if let Some((reply, _)) = pending_stop.take() {
                                    let _ = reply.send(Err(format!("Session aborted: {e}")));
                                }
                                return SessionEnd::Aborted(format!(
                                    "Transcription failed {MAX_CONSECUTIVE_FRAME_ERRORS} frames in a row: {e}"
                                ));
                            }
                        }
                    }
                    summary.frames_processed += 1;
                    health.note_frame();
                    if crate::debug::transcription_debug_enabled()
                        && summary.frames_processed.is_multiple_of(50)
                    {
                        let frame_duration =
                            chunk_size as f64 / engine.audio_requirements().sample_rate_hz as f64;
                        debug!(
                            "Processed {} frames ({:.1}s)",
                            summary.frames_processed,
                            summary.frames_processed as f64 * frame_duration,
                        );
                    }
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return SessionEnd::AudioGone;
            }
        }
    }
}

/// Drain remaining audio, run every buffered frame through the engine (no VAD
/// gating: everything already captured must reach the engine), feed the
/// tail(s), and flush. Every stage's segments go through `emit_filtered`.
fn finish_session(
    audio_rx: &Receiver<AudioMessage>,
    engine: &mut dyn TranscriptionEngine,
    on_segment: &SegmentCallback,
    session_id: u64,
    text_filters: &TextFilterChain,
    mode: &mut dyn SessionMode,
    summary: &mut SessionSummary,
) {
    let chunk_size = engine.audio_requirements().chunk_size_samples as usize;

    // Collect anything still queued (normally nothing — EndOfStream is the
    // last message — but the EOS-timeout path can leave chunks behind).
    let mut drained = 0usize;
    while let Ok(msg) = audio_rx.try_recv() {
        match msg {
            AudioMessage::Chunk(chunk) if chunk.session_id == session_id => {
                mode.ingest(chunk);
                drained += 1;
            }
            AudioMessage::Chunk(_) => summary.skipped_chunks += 1,
            AudioMessage::EndOfStream { .. } => {}
        }
    }
    if drained > 0 && crate::debug::transcription_debug_enabled() {
        debug!("Drained {drained} remaining audio chunks on stop");
    }

    // Process all buffered audio through the engine (frame by frame).
    while mode.frame_ready(chunk_size) {
        match mode.finish_frame(engine, chunk_size) {
            Ok(segments) => {
                for seg in segments {
                    if crate::debug::transcription_debug_enabled() {
                        debug!("Drain segment: {:?} final={}", seg.text, seg.is_final);
                    }
                    emit_filtered(engine, text_filters, seg, on_segment);
                }
            }
            Err(e) => {
                error!("Transcribe error during drain: {e}");
                break;
            }
        }
        summary.frames_processed += 1;
    }
    // Process any remaining partial tail(s).
    match mode.finish_tail(engine) {
        Ok(segments) => {
            for seg in segments {
                emit_filtered(engine, text_filters, seg, on_segment);
            }
        }
        Err(e) => error!("Transcribe error on partial frame: {e}"),
    }

    // Flush engine (e.g. feeds silence to extract remaining buffered tokens)
    match catch_engine(|| engine.flush()) {
        Ok(segments) => {
            if crate::debug::transcription_debug_enabled() {
                debug!("Flush: {} segments", segments.len());
            }
            for seg in segments {
                emit_filtered(engine, text_filters, seg, on_segment);
            }
        }
        Err(e) => error!("Flush error: {e}"),
    }
}

/// Run a fallible engine call, converting a panic into an `Err` so a bug in the
/// engine/Metal/candle path degrades to a logged error instead of unwinding out
/// of the actor thread (or, under abort, taking down the whole process).
fn catch_engine<T, E: std::string::ToString>(
    f: impl FnOnce() -> Result<T, E>,
) -> Result<T, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(inner) => inner.map_err(|e| e.to_string()),
        Err(_) => Err("engine panicked".to_string()),
    }
}

/// Normalize engine-specific tokens, then apply text filter chain before emitting.
fn emit_filtered(
    engine: &dyn TranscriptionEngine,
    text_filters: &TextFilterChain,
    mut segment: TranscriptionSegment,
    on_segment: &SegmentCallback,
) {
    // Step 1: engine-specific normalization (strip [_TT_], ▁, etc.)
    segment.text = engine.normalize_text(&segment.text);
    // Step 2: shared text filter chain (filler, stutter, dictionary, whitespace)
    segment.text = text_filters.apply(&segment.text);
    if !segment.text.is_empty() {
        on_segment(segment);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use crossbeam_channel::{Sender, unbounded};

    use crate::audio::{AudioChunk, AudioMessage};
    use crate::constants::MIMI_FRAME_SIZE;
    use crate::engine::mock::MockEngine;
    use crate::engine::{Speaker, TranscriptionSegment, default_transcription_profile};
    use crate::filter::PipelineConfig;

    use super::{EngineActorHandle, SessionConfig};

    /// Helper: create a disabled filter config for tests (no VAD, no text filters).
    fn noop_filter_config() -> PipelineConfig {
        PipelineConfig {
            vad_enabled: false,
            vad_model_path: None,
            filler_removal_enabled: false,
            stutter_collapse_enabled: false,
            dictionary_correction_enabled: false,
        }
    }

    fn session_config() -> SessionConfig {
        SessionConfig {
            pipeline_config: noop_filter_config(),
            dictionary_entries: vec![],
            diarize: false,
        }
    }

    /// Helper: create a TranscriptionSegment with the given text.
    fn seg(text: &str) -> TranscriptionSegment {
        TranscriptionSegment {
            text: text.to_string(),
            start_time: 0.0,
            end_time: 0.0,
            is_final: false,
            language: None,
            confidence: None,
            speaker: None,
        }
    }

    /// Helper: create a chunk message with MIMI_FRAME_SIZE samples for the given session.
    fn audio_chunk(session_id: u64) -> AudioMessage {
        AudioMessage::Chunk(AudioChunk {
            session_id,
            samples: vec![0.0f32; MIMI_FRAME_SIZE],
            captured_at: std::time::Instant::now(),
            speaker: None,
        })
    }

    fn end_of_stream(session_id: u64) -> AudioMessage {
        AudioMessage::EndOfStream { session_id }
    }

    /// Spawn an actor whose factory hands out the given pre-configured mock
    /// (built on the actor thread, mirroring how production uses create_engine).
    fn spawn_with_mock(mock: MockEngine) -> (EngineActorHandle, Sender<AudioMessage>) {
        let (audio_tx, audio_rx) = unbounded();
        let cell = Mutex::new(Some(mock));
        let actor = EngineActorHandle::spawn(
            audio_rx,
            Arc::new(std::sync::atomic::AtomicU64::new(0)),
            Box::new(move |_profile| {
                cell.lock()
                    .unwrap()
                    .take()
                    .map(|m| Box::new(m) as Box<dyn crate::engine::TranscriptionEngine>)
                    .ok_or_else(|| "mock engine already taken".to_string())
            }),
        )
        .expect("spawn actor");
        actor
            .load_model(default_transcription_profile(), PathBuf::from("/tmp"))
            .expect("load mock model");
        (actor, audio_tx)
    }

    fn collecting_callback() -> (
        Arc<Mutex<Vec<TranscriptionSegment>>>,
        super::SegmentCallback,
    ) {
        let collected: Arc<Mutex<Vec<TranscriptionSegment>>> = Arc::new(Mutex::new(Vec::new()));
        let cb_ref = Arc::clone(&collected);
        let cb: super::SegmentCallback = Box::new(move |s| {
            cb_ref.lock().unwrap().push(s);
        });
        (collected, cb)
    }

    #[test]
    fn actor_shutdown_is_idempotent() {
        let (actor, _audio_tx) = spawn_with_mock(MockEngine::new());
        actor.shutdown().expect("first shutdown");
        actor.shutdown().expect("second shutdown");
    }

    /// A non-silent chunk tagged with its source, so the mock's transcribe_dual
    /// emits a segment for that lane.
    fn audio_chunk_from(session_id: u64, speaker: Option<Speaker>) -> AudioMessage {
        AudioMessage::Chunk(AudioChunk {
            session_id,
            samples: vec![0.5f32; MIMI_FRAME_SIZE],
            captured_at: std::time::Instant::now(),
            speaker,
        })
    }

    #[test]
    fn diarized_session_tags_segments_by_source() {
        // One batched engine (MockEngine reports supports_diarization() = true).
        let (actor, audio_tx) = spawn_with_mock(MockEngine::new());

        let (collected, cb) = collecting_callback();
        let cfg = SessionConfig {
            pipeline_config: noop_filter_config(),
            dictionary_entries: vec![],
            diarize: true,
        };
        actor.start_session(1, cfg, cb).expect("start diarized");

        for _ in 0..3 {
            audio_tx
                .send(audio_chunk_from(1, Some(Speaker::Me)))
                .unwrap();
            audio_tx
                .send(audio_chunk_from(1, Some(Speaker::Them)))
                .unwrap();
        }
        audio_tx.send(end_of_stream(1)).unwrap();
        actor
            .stop_session(Duration::from_secs(2))
            .expect("stop diarized");

        let segments = collected.lock().unwrap();
        assert!(
            segments
                .iter()
                .any(|s| s.speaker == Some(Speaker::Me) && s.text == "me-speaks"),
            "expected a Me segment, got: {:?}",
            segments
                .iter()
                .map(|s| (&s.text, s.speaker))
                .collect::<Vec<_>>()
        );
        assert!(
            segments
                .iter()
                .any(|s| s.speaker == Some(Speaker::Them) && s.text == "them-speaks"),
            "expected a Them segment, got: {:?}",
            segments
                .iter()
                .map(|s| (&s.text, s.speaker))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn actor_processes_audio_frames() {
        let mock = MockEngine::new().with_transcribe_response(Ok(vec![seg("hello")]), 3);
        let (actor, audio_tx) = spawn_with_mock(mock);

        let (collected, cb) = collecting_callback();
        actor.start_session(1, session_config(), cb).expect("start");

        for _ in 0..3 {
            audio_tx.send(audio_chunk(1)).unwrap();
        }
        audio_tx.send(end_of_stream(1)).unwrap();

        actor
            .stop_session(Duration::from_secs(2))
            .expect("stop session");

        let segments = collected.lock().unwrap();
        assert!(
            segments.iter().any(|s| s.text == "hello"),
            "Expected at least one 'hello' segment, got: {:?}",
            segments.iter().map(|s| &s.text).collect::<Vec<_>>()
        );
    }

    #[test]
    fn actor_start_stop_cycle() {
        let (actor, audio_tx) = spawn_with_mock(MockEngine::new());

        actor
            .start_session(1, session_config(), Box::new(|_| {}))
            .expect("start");
        audio_tx.send(audio_chunk(1)).unwrap();
        audio_tx.send(end_of_stream(1)).unwrap();

        actor
            .stop_session(Duration::from_secs(2))
            .expect("stop should complete");
    }

    #[test]
    fn actor_flush_on_stop() {
        let mock = MockEngine::new().with_flush_response(Ok(vec![seg("flushed")]));
        let (actor, audio_tx) = spawn_with_mock(mock);

        let (collected, cb) = collecting_callback();
        actor.start_session(1, session_config(), cb).expect("start");
        audio_tx.send(end_of_stream(1)).unwrap();

        actor.stop_session(Duration::from_secs(2)).expect("stop");

        let segments = collected.lock().unwrap();
        assert!(
            segments.iter().any(|s| s.text == "flushed"),
            "Expected flush segment 'flushed', got: {:?}",
            segments.iter().map(|s| &s.text).collect::<Vec<_>>()
        );
    }

    #[test]
    fn actor_drains_all_audio_sent_before_stop() {
        let mock = MockEngine::new().with_transcribe_response(Ok(vec![seg("drained")]), 5);
        let (actor, audio_tx) = spawn_with_mock(mock);

        let (collected, cb) = collecting_callback();
        actor.start_session(1, session_config(), cb).expect("start");

        // Send multiple chunks then EndOfStream, then immediately stop.
        // Stop waits for EndOfStream, so every chunk must be transcribed —
        // deterministically, without sleeps.
        for _ in 0..5 {
            audio_tx.send(audio_chunk(1)).unwrap();
        }
        audio_tx.send(end_of_stream(1)).unwrap();

        let summary = actor.stop_session(Duration::from_secs(2)).expect("stop");
        assert_eq!(summary.frames_processed, 5);

        let segments = collected.lock().unwrap();
        let drained_count = segments.iter().filter(|s| s.text == "drained").count();
        assert_eq!(
            drained_count, 5,
            "All chunks sent before EndOfStream must be transcribed"
        );
    }

    #[test]
    fn actor_stop_before_audio_waits_for_end_of_stream() {
        let mock = MockEngine::new().with_transcribe_response(Ok(vec![seg("late")]), 2);
        let (actor, audio_tx) = spawn_with_mock(mock);

        let (collected, cb) = collecting_callback();
        actor.start_session(1, session_config(), cb).expect("start");

        // Request stop on a separate thread BEFORE any audio arrives, then
        // deliver audio + EndOfStream. The stop must wait and still process it.
        let stopper = {
            let audio_tx = audio_tx.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                audio_tx.send(audio_chunk(1)).unwrap();
                audio_tx.send(audio_chunk(1)).unwrap();
                audio_tx.send(end_of_stream(1)).unwrap();
            })
        };

        let summary = actor.stop_session(Duration::from_secs(3)).expect("stop");
        stopper.join().unwrap();

        assert_eq!(summary.frames_processed, 2);
        let segments = collected.lock().unwrap();
        assert_eq!(segments.iter().filter(|s| s.text == "late").count(), 2);
    }

    #[test]
    fn actor_multiple_sessions() {
        let mock = MockEngine::new()
            .with_transcribe_response(Ok(vec![seg("s1")]), 2)
            .with_transcribe_response(Ok(vec![seg("s2")]), 2)
            .with_flush_response(Ok(vec![]))
            .with_flush_response(Ok(vec![]));
        let (actor, audio_tx) = spawn_with_mock(mock);

        // --- Session 1 ---
        let (collected1, cb1) = collecting_callback();
        actor
            .start_session(1, session_config(), cb1)
            .expect("start session 1");
        audio_tx.send(audio_chunk(1)).unwrap();
        audio_tx.send(end_of_stream(1)).unwrap();
        actor
            .stop_session(Duration::from_secs(2))
            .expect("stop session 1");

        // --- Session 2 ---
        let (collected2, cb2) = collecting_callback();
        actor
            .start_session(2, session_config(), cb2)
            .expect("start session 2");
        audio_tx.send(audio_chunk(2)).unwrap();
        audio_tx.send(end_of_stream(2)).unwrap();
        actor
            .stop_session(Duration::from_secs(2))
            .expect("stop session 2");

        assert!(
            !collected1.lock().unwrap().is_empty(),
            "Session 1 should have produced segments"
        );
        assert!(
            !collected2.lock().unwrap().is_empty(),
            "Session 2 should have produced segments"
        );
    }

    #[test]
    fn actor_ignores_stale_session_audio() {
        let mock = MockEngine::new()
            .with_transcribe_response(Ok(vec![seg("fresh")]), 3)
            .with_flush_response(Ok(vec![]));
        let (actor, audio_tx) = spawn_with_mock(mock);

        let (collected, cb) = collecting_callback();
        actor.start_session(2, session_config(), cb).expect("start");

        // Stale chunks (session_id=1), then one valid chunk (session_id=2)
        for _ in 0..3 {
            audio_tx.send(audio_chunk(1)).unwrap();
        }
        audio_tx.send(audio_chunk(2)).unwrap();
        audio_tx.send(end_of_stream(2)).unwrap();

        let summary = actor.stop_session(Duration::from_secs(2)).expect("stop");
        assert_eq!(summary.skipped_chunks, 3, "stale chunks should be counted");

        let segments = collected.lock().unwrap();
        let fresh_count = segments.iter().filter(|s| s.text == "fresh").count();
        assert_eq!(
            fresh_count, 1,
            "Expected exactly 1 segment from session-2 audio, got {fresh_count} (stale audio should be ignored)"
        );
    }

    #[test]
    fn actor_start_without_model_fails() {
        let (audio_tx, audio_rx) = unbounded::<AudioMessage>();
        let _keep = audio_tx;
        let actor = EngineActorHandle::spawn(
            audio_rx,
            Arc::new(std::sync::atomic::AtomicU64::new(0)),
            Box::new(|_| Err("no engine in this test".into())),
        )
        .expect("spawn");

        let err = actor
            .start_session(1, session_config(), Box::new(|_| {}))
            .expect_err("start without model must fail");
        assert!(err.contains("No model loaded"), "got: {err}");
    }

    #[test]
    fn actor_aborts_after_consecutive_errors_and_recovers() {
        let mock = MockEngine::new().with_transcribe_response(
            Err(crate::engine::EngineError::InferenceError(
                "gpu lost".into(),
            )),
            super::MAX_CONSECUTIVE_FRAME_ERRORS as usize,
        );
        let (actor, audio_tx) = spawn_with_mock(mock);

        actor
            .start_session(1, session_config(), Box::new(|_| {}))
            .expect("start");
        for _ in 0..super::MAX_CONSECUTIVE_FRAME_ERRORS {
            audio_tx.send(audio_chunk(1)).unwrap();
        }
        audio_tx.send(end_of_stream(1)).unwrap();

        // Depending on timing, stop either lands after the abort (idle →
        // default summary) or while aborting (Err mentioning the abort).
        match actor.stop_session(Duration::from_secs(3)) {
            Ok(summary) => assert_eq!(summary.frames_processed, 0),
            Err(e) => assert!(e.contains("aborted"), "unexpected error: {e}"),
        }

        // The actor must remain usable: a new session starts and stops cleanly
        // (the mock's error queue is exhausted, so transcribe returns Ok).
        actor
            .start_session(2, session_config(), Box::new(|_| {}))
            .expect("recover with a new session");
        audio_tx.send(audio_chunk(2)).unwrap();
        audio_tx.send(end_of_stream(2)).unwrap();
        actor
            .stop_session(Duration::from_secs(2))
            .expect("stop recovered session");
    }

    #[test]
    fn actor_stop_while_idle_is_ok() {
        let (actor, _audio_tx) = spawn_with_mock(MockEngine::new());
        actor
            .stop_session(Duration::from_secs(2))
            .expect("stop while idle should be idempotent");
    }
}
