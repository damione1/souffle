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

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;
use tauri_specta::Event;
use tracing::{debug, error, info, warn};

use crate::app_events::{MeetingIdle, MeetingIdleReason, PipelineError, PipelineErrorScope};
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
use super::health::{SessionHealth, StallAction, StallRecovery};
use super::idle::{MeetingIdleConfig, MeetingIdleMonitor};

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
    /// Ephemeral correction terms for this session only (participant names,
    /// meeting jargon); never persisted to the user's dictionary.
    pub session_terms: Vec<String>,
    /// Diarized meeting: run a second engine so the mic (Me) and system audio
    /// (Them) legs are transcribed separately and segments are speaker-tagged.
    pub diarize: bool,
    /// Meeting-only auto-stop detection (silence + max duration). `None` for
    /// dictation sessions, where "meeting is over" doesn't apply.
    pub idle_config: Option<MeetingIdleConfig>,
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
    /// Reconfigure the idle-unload timeout. `None` disables it. Fire-and-forget.
    SetUnloadTimeout(Option<Duration>),
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
    /// `audio_gone_reason` is shared with the audio capture thread: when the
    /// audio channel disconnects, the actor takes whatever reason capture
    /// left there (e.g. an unrecoverable mic loss) instead of assuming a
    /// generic failure.
    pub fn spawn(
        audio_rx: Receiver<AudioMessage>,
        dropped_counter: Arc<AtomicU64>,
        audio_gone_reason: Arc<Mutex<Option<String>>>,
        factory: EngineFactory,
    ) -> Result<Self, String> {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<EngineCommand>();

        let handle = std::thread::Builder::new()
            .name("engine-actor".into())
            .spawn(move || {
                crate::thread_qos::set_current_thread_qos(crate::thread_qos::ThreadQos::UserInitiated);
                EngineActor {
                    cmd_rx,
                    audio_rx,
                    factory,
                    engine: None,
                    app: None,
                    dropped_counter,
                    audio_gone_reason,
                    unload_timeout: None,
                    idle_since: None,
                    state_fresh_for: None,
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

    /// Reconfigure the idle-unload timeout; `0` disables it. Fire-and-forget:
    /// the actor picks it up on its next loop iteration.
    pub fn set_unload_timeout(&self, minutes: u32) {
        let duration = (minutes > 0).then(|| Duration::from_secs(u64::from(minutes) * 60));
        let _ = self.cmd_tx.send(EngineCommand::SetUnloadTimeout(duration));
    }

    /// Test-only: set the idle-unload timeout directly as a `Duration`, so
    /// tests don't have to wait out real minutes.
    #[cfg(test)]
    pub fn set_unload_timeout_for_test(&self, duration: Duration) {
        let _ = self
            .cmd_tx
            .send(EngineCommand::SetUnloadTimeout(Some(duration)));
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
    /// Reason the audio capture thread set right before it exits after an
    /// unrecoverable failure (e.g. mic loss with no other source). Read
    /// (and cleared) when the audio channel disconnects; falls back to a
    /// generic message when empty (audio thread panicked or exited for some
    /// other reason).
    audio_gone_reason: Arc<Mutex<Option<String>>>,
    /// Idle-unload timeout; `None` means "never unload". Reconfigured via
    /// `EngineCommand::SetUnloadTimeout`.
    unload_timeout: Option<Duration>,
    /// When the engine last became idle (loaded, no session active). `None`
    /// while a session is running or no engine is loaded.
    idle_since: Option<Instant>,
    /// Diarize mode the engine's streaming state is currently reset/fresh
    /// for (`Some(false)` = single-stream, `Some(true)` = diarized), or
    /// `None` if the state is stale and needs a reset before the next
    /// session. Set by the idle pre-warm after a session ends and by the
    /// initial load; cleared as soon as a session starts consuming it.
    state_fresh_for: Option<bool>,
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
            // While the engine is loaded, idle, and an unload timeout is
            // configured, wait only until the deadline instead of blocking
            // forever. Everything else (an active session, no engine, no
            // timeout) keeps the original plain `recv()`.
            let cmd = if let Some(deadline) = self.idle_deadline() {
                let now = Instant::now();
                if now >= deadline {
                    self.handle_idle_timeout();
                    continue;
                }
                match self.cmd_rx.recv_timeout(deadline - now) {
                    Ok(cmd) => cmd,
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => {
                        warn!("Engine actor command channel disconnected, exiting");
                        break;
                    }
                }
            } else {
                match self.cmd_rx.recv() {
                    Ok(cmd) => cmd,
                    Err(_) => {
                        warn!("Engine actor command channel disconnected, exiting");
                        break;
                    }
                }
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
                    self.idle_since = None;
                    let mut pending_unload_timeout: Option<Option<Duration>> = None;
                    let end = with_autorelease_pool(|| {
                        self.handle_session(
                            session_id,
                            config,
                            on_segment,
                            reply,
                            &mut pending_unload_timeout,
                        )
                    });
                    // A SetUnloadTimeout received mid-session was consumed by
                    // the session loop's command drain; apply it now instead
                    // of losing it.
                    if let Some(duration) = pending_unload_timeout {
                        self.unload_timeout = duration;
                    }
                    // Whether the session actually reached the engine (so its
                    // state needs pre-warming back to dictation-ready before
                    // the next start_session). Setup failures never touched
                    // audio, so there's nothing to re-warm.
                    let mut ran = true;
                    match end {
                        SessionEnd::Stopped(done, summary) => {
                            let _ = done.send(Ok(summary));
                            info!("Inference session {session_count} ended");
                        }
                        SessionEnd::SetupFailed => {
                            warn!("Inference session {session_count} failed during setup");
                            ran = false;
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
                            // The capture thread leaves a specific reason here
                            // before it exits deliberately (e.g. unrecoverable
                            // mic loss); fall back to a generic message when
                            // it's empty (a genuine panic or unexpected exit).
                            let reason = self
                                .audio_gone_reason
                                .lock()
                                .ok()
                                .and_then(|mut guard| guard.take())
                                .unwrap_or_else(|| {
                                    "Audio capture stopped unexpectedly".to_string()
                                });
                            self.handle_session_abort(reason);
                        }
                    }
                    // Pre-warm AFTER the stop reply above so stop latency is
                    // unchanged; the caller is already unblocked by now.
                    if ran {
                        self.prewarm_single_stream();
                    }
                    if self.engine.is_some() {
                        self.idle_since = Some(Instant::now());
                    }
                }
                EngineCommand::StopSession { reply } => {
                    // Already idle — stop is idempotent.
                    let _ = reply.send(Ok(SessionSummary::default()));
                }
                EngineCommand::DebugTranscribe { samples, reply } => {
                    let result = with_autorelease_pool(|| self.handle_debug_transcribe(&samples));
                    let _ = reply.send(result);
                    if self.engine.is_some() {
                        self.idle_since = Some(Instant::now());
                    }
                }
                EngineCommand::SetUnloadTimeout(duration) => {
                    self.unload_timeout = duration;
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
        self.idle_since = None;
        self.state_fresh_for = None;
    }

    /// Deadline at which the currently loaded, idle model should be
    /// unloaded, or `None` if no unload is scheduled: no engine loaded, no
    /// timeout configured, or a session is active (which clears `idle_since`).
    fn idle_deadline(&self) -> Option<Instant> {
        self.engine.as_ref()?;
        let timeout = self.unload_timeout?;
        let idle_since = self.idle_since?;
        Some(idle_since + timeout)
    }

    /// Unload the model after it has sat idle past the configured timeout,
    /// and tell `AppState` so the state machine (and therefore the frontend)
    /// reflects reality: the next recording start must reload through the
    /// normal load flow instead of finding a silently-vanished engine.
    fn handle_idle_timeout(&mut self) {
        info!(
            timeout = ?self.unload_timeout,
            "Unloading idle transcription model to reclaim memory"
        );
        with_autorelease_pool(|| self.drop_engine());
        if let Some(app) = &self.app {
            app.state::<crate::state::AppState>().unload_idle_model();
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
        // No session is active right after a load: start the idle clock.
        self.idle_since = Some(Instant::now());
        // build_loaded_model always constructs single-stream (batch size 1)
        // state, so the freshly loaded engine is already pre-warmed for a
        // dictation start with no extra reset needed.
        self.state_fresh_for = Some(false);
        Ok(info)
    }

    fn handle_debug_transcribe(
        &mut self,
        samples: &[f32],
    ) -> Result<Vec<TranscriptionSegment>, String> {
        let engine = self.engine.as_mut().ok_or("No model loaded")?;
        engine.reset_state().map_err(|e| e.to_string())?;
        let result = engine.transcribe(samples, None).map_err(|e| e.to_string());
        // Debug transcribe resets and then feeds arbitrary audio outside the
        // normal session lifecycle; don't let a stale freshness claim skip a
        // real reset on the next session start.
        self.state_fresh_for = None;
        result
    }

    /// After a session ends, reset the engine to single-stream (dictation)
    /// state while idle, so the next `start_session` for dictation can skip
    /// the reset entirely. Meeting starts (diarize=true) still pay the reset
    /// cost, same as today, since they begin from an explicit UI click.
    fn prewarm_single_stream(&mut self) {
        if self.state_fresh_for == Some(false) {
            return; // already fresh for single-stream
        }
        let Some(engine) = self.engine.as_mut() else {
            return;
        };
        engine.set_diarization(false);
        let start = Instant::now();
        match engine.reset_state() {
            Ok(()) => {
                info!(
                    duration_ms = start.elapsed().as_millis(),
                    "Pre-warmed engine state for dictation while idle"
                );
                self.state_fresh_for = Some(false);
            }
            Err(e) => {
                warn!("Idle pre-warm reset_state failed: {e}");
                self.state_fresh_for = None;
            }
        }
    }

    /// Prepare and run one transcription session. Replies on `reply` once the
    /// engine is reset and filters are built (success), or with the error.
    fn handle_session(
        &mut self,
        session_id: u64,
        config: SessionConfig,
        on_segment: SegmentCallback,
        reply: Sender<Result<EngineInfo, String>>,
        pending_unload_timeout: &mut Option<Option<Duration>>,
    ) -> SessionEnd {
        let session_start = Instant::now();
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

        // The idle pre-warm (see `prewarm_single_stream`) keeps the engine
        // state reset for dictation between sessions; skip the reset here
        // (a full model rebuild for Kyutai, on the order of seconds) when
        // the requested mode already matches what's pre-warmed. A meeting
        // start (diarize=true) after a dictation pre-warm still pays it,
        // since that flow starts from a UI click and tolerates the latency.
        if self.state_fresh_for == Some(diarize) {
            info!(session_id, diarize, "Engine state pre-warmed, skipping reset_state");
        } else {
            let reset_start = Instant::now();
            // Reset rebuilds streaming state with the right batch size (2 when
            // diarized, 1 otherwise).
            if let Err(e) = engine.reset_state() {
                let _ = reply.send(Err(format!("State reset: {e}")));
                self.state_fresh_for = None;
                return SessionEnd::SetupFailed;
            }
            info!(
                session_id,
                diarize,
                duration_ms = reset_start.elapsed().as_millis(),
                "Engine reset_state completed"
            );
        }
        // The session is about to consume this state; it stops being fresh
        // the moment real audio flows. Re-established once the session ends.
        self.state_fresh_for = None;

        let info = EngineInfo {
            audio: engine.audio_requirements(),
            mic_gain: engine.mic_gain(),
        };
        let sample_rate = info.audio.sample_rate_hz;

        // Filter chains are built on this thread so ONNX Runtime (Silero VAD)
        // initialization shares the thread with engine Metal work.
        let text_filters = build_text_filters(
            &config.pipeline_config,
            config.dictionary_entries,
            &config.session_terms,
        );

        let mut health = SessionHealth::start(session_id, Arc::clone(&self.dropped_counter));
        let idle_monitor = config
            .idle_config
            .map(|idle_config| MeetingIdleMonitor::new(idle_config, Instant::now()));
        let engine = self.engine.as_mut().expect("engine present: checked above");

        // No Silero VAD in diarized mode: the two lanes must step together
        // every frame to stay aligned in the batch, so we can't drop a silent
        // frame from one side. Kyutai's own semantic VAD handles pauses.
        let mut mode: Box<dyn SessionMode> = if diarize {
            Box::new(DiarizedMode::new())
        } else {
            let filters_start = Instant::now();
            let audio_filters = build_audio_filters(&config.pipeline_config, sample_rate);
            info!(
                session_id,
                duration_ms = filters_start.elapsed().as_millis(),
                "Audio filter chain built"
            );
            // Bounded window (in engine frames) to keep feeding the engine
            // after VAD gates speech, so the emission-delayed tail word
            // drains instead of staying stuck behind the next utterance.
            let frames_per_second = sample_rate as f64 / info.audio.chunk_size_samples as f64;
            let drain_window_frames =
                ((engine.emission_delay_seconds() + 0.5) * frames_per_second).ceil() as usize;
            Box::new(SingleMode::new(audio_filters, drain_window_frames))
        };

        // Caller may now start audio capture.
        let _ = reply.send(Ok(info));
        info!(
            session_id,
            duration_ms = session_start.elapsed().as_millis(),
            "start_session setup completed"
        );

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
            pending_unload_timeout,
            idle_monitor,
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
    /// Frames still fed to the engine after VAD gated them, to drain the
    /// engine's emission delay once speech stops (counted separately from
    /// `vad_skipped` so the health counters stay meaningful).
    vad_drained: u64,
    /// How many consecutive VAD-negative frames to keep feeding the engine
    /// after the last VAD-positive frame, before actually gating. Sized to
    /// the engine's emission delay so a trailing word/punctuation still
    /// surfaces during a pause instead of waiting for the next utterance.
    drain_window_frames: usize,
    /// Consecutive VAD-negative frames seen since the last VAD-positive one.
    frames_since_speech: usize,
}

impl SingleMode {
    fn new(audio_filters: AudioFilterChain, drain_window_frames: usize) -> Self {
        Self {
            buffer: Vec::new(),
            audio_filters,
            vad_skipped: 0,
            vad_drained: 0,
            drain_window_frames,
            frames_since_speech: 0,
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
        let speech = self.audio_filters.process(&frame);
        if speech {
            self.frames_since_speech = 0;
        } else {
            self.frames_since_speech += 1;
            if self.frames_since_speech > self.drain_window_frames {
                self.vad_skipped += 1;
                return Ok(None); // Past the drain window, no speech: skip.
            }
            self.vad_drained += 1;
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
            vad_drained = self.vad_drained,
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
    pending_unload_timeout: &mut Option<Option<Duration>>,
    mut idle_monitor: Option<MeetingIdleMonitor>,
) -> SessionEnd {
    let chunk_size = engine.audio_requirements().chunk_size_samples as usize;
    let mut summary = SessionSummary::default();
    let mut eos_received = false;
    let mut consecutive_errors: u32 = 0;
    // Drives the stall recovery ladder (in-place reset, then give up) from
    // continuous stalled health snapshots. See `StallRecovery` for the
    // known limitation: an engine call that blocks forever never returns
    // control here, so this only covers "loop alive, no frames" stalls.
    let mut stall_recovery = StallRecovery::new();
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
        // Periodic health snapshot to the frontend, and to the stall
        // recovery ladder.
        if let Some(snapshot) = health.tick(audio_rx.len()) {
            let action = stall_recovery.on_snapshot(snapshot.status, Instant::now());
            if let Some(app) = app {
                let _ = snapshot.emit(app);
            }
            match action {
                Some(StallAction::Reset) => {
                    warn!(
                        "Session {session_id} stalled for 30s with no frames processed; \
                         attempting in-place engine reset"
                    );
                    // Match the panic protection already used for engine
                    // calls in this loop (mode.step / catch_engine below): a
                    // broken engine must not take the actor thread down
                    // while we try to recover it.
                    match catch_engine(|| engine.reset_state()) {
                        Ok(()) => info!("Session {session_id}: stall recovery reset succeeded"),
                        Err(e) => {
                            warn!("Session {session_id}: stall recovery reset failed: {e}")
                        }
                    }
                }
                Some(StallAction::Abort) => {
                    let message = "The transcription engine stopped responding; \
                                    the recording so far was saved."
                        .to_string();
                    if let Some((reply, _)) = pending_stop.take() {
                        let _ = reply.send(Err(message.clone()));
                    }
                    return SessionEnd::Aborted(message);
                }
                None => {}
            }
        }

        // Meeting-only: has the meeting probably ended (silence / max duration)?
        if let Some(monitor) = idle_monitor.as_mut()
            && let Some(signal) = monitor.tick(Instant::now())
            && let Some(app) = app
        {
            let _ = MeetingIdle {
                reason: signal.reason,
                idle_seconds: signal.idle_seconds,
                threshold_seconds: signal.threshold_seconds,
            }
            .emit(app);
            if signal.first {
                notify_meeting_idle(app, signal.reason);
            }
        }

        if last_heartbeat.elapsed() >= HEARTBEAT {
            last_heartbeat = Instant::now();
            mode.log_heartbeat(
                session_id,
                summary.frames_processed,
                segments_emitted,
                audio_rx.len(),
            );
            if let Some(stats) = engine.context_window_stats() {
                info!(
                    session_id,
                    context_frames = stats.context_frames,
                    frames_since_refresh = stats.frames_since_refresh,
                    refresh_count = stats.refresh_count,
                    "ASR context window heartbeat"
                );
            }
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
            // No reply channel to report "busy" on; remember the latest
            // value and let the caller apply it once the session ends,
            // rather than silently dropping the setting change.
            Ok(EngineCommand::SetUnloadTimeout(duration)) => {
                *pending_unload_timeout = Some(duration);
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
                                if emit_filtered(engine, text_filters, seg, on_segment)
                                    && let Some(monitor) = idle_monitor.as_mut()
                                {
                                    monitor.note_segment(Instant::now());
                                }
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

/// Normalize engine-specific tokens, then apply text filter chain before
/// emitting. Returns whether the filtered text was non-empty (i.e. an actual
/// segment reached `on_segment`), which the meeting idle monitor uses as its
/// speech-activity signal.
fn emit_filtered(
    engine: &dyn TranscriptionEngine,
    text_filters: &TextFilterChain,
    mut segment: TranscriptionSegment,
    on_segment: &SegmentCallback,
) -> bool {
    // Step 1: engine-specific normalization (strip [_TT_], ▁, etc.)
    segment.text = engine.normalize_text(&segment.text);
    // Step 2: shared text filter chain (filler, stutter, dictionary, whitespace)
    segment.text = text_filters.apply(&segment.text);
    if segment.text.is_empty() {
        return false;
    }
    on_segment(segment);
    true
}

/// System notification for the first crossing of an idle signal only;
/// re-signals (throttled webview convergence) must not spam the OS.
/// Informational only: no action buttons, matching the calendar reminder's
/// notification (action buttons/click callbacks are unreliable on macOS).
fn notify_meeting_idle(app: &tauri::AppHandle, reason: MeetingIdleReason) {
    let db = &app.state::<crate::state::AppState>().db;
    let locale = crate::settings::AppSettings::load(db)
        .map(|settings| settings.locale)
        .unwrap_or_default();
    let french = locale.starts_with("fr");

    let (title, body) = match reason {
        MeetingIdleReason::Silence => (
            if french { "Réunion probablement terminée" } else { "Meeting seems to be over" },
            if french {
                "Aucune parole détectée depuis un moment. L'enregistrement va bientôt s'arrêter."
            } else {
                "No speech detected for a while. Recording will stop soon."
            },
        ),
        MeetingIdleReason::MaxDuration => (
            if french { "Durée maximale atteinte" } else { "Maximum duration reached" },
            if french {
                "L'enregistrement de la réunion a atteint la durée maximale et va s'arrêter."
            } else {
                "The meeting recording hit its maximum duration and is stopping."
            },
        ),
    };

    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        warn!("Meeting idle notification failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use crossbeam_channel::{Sender, unbounded};

    use crate::audio::{AudioChunk, AudioMessage};
    use crate::constants::MIMI_FRAME_SIZE;
    use crate::engine::mock::MockEngine;
    use crate::engine::{Speaker, TranscriptionSegment, default_transcription_profile};
    use crate::filter::PipelineConfig;

    use super::{EngineActorHandle, SessionConfig, SessionMode, SingleMode};

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
            session_terms: vec![],
            diarize: false,
            idle_config: None,
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
            Arc::new(Mutex::new(None)),
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
            session_terms: vec![],
            diarize: true,
            idle_config: None,
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
            Arc::new(Mutex::new(None)),
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

    /// Poll `cond` until it's true or `timeout` elapses, so idle-unload tests
    /// don't rely on a single fixed sleep racing the actor thread.
    fn wait_for(cond: impl Fn() -> bool, timeout: Duration) -> bool {
        let start = Instant::now();
        loop {
            if cond() {
                return true;
            }
            if start.elapsed() >= timeout {
                return cond();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn idle_timeout_unloads_engine_after_inactivity() {
        let mock = MockEngine::new();
        let unload_count = mock.unload_count_handle();
        // spawn_with_mock's initial load_model call already starts the idle
        // clock, so configuring a tiny timeout is enough to trigger unload.
        let (actor, _audio_tx) = spawn_with_mock(mock);

        actor.set_unload_timeout_for_test(Duration::from_millis(50));

        assert!(
            wait_for(|| unload_count.load(Ordering::SeqCst) >= 1, Duration::from_secs(2)),
            "engine should unload once the idle timeout elapses"
        );
    }

    #[test]
    fn idle_timeout_set_during_session_applies_after_it_ends() {
        let mock = MockEngine::new();
        let unload_count = mock.unload_count_handle();
        let (actor, audio_tx) = spawn_with_mock(mock);

        actor
            .start_session(1, session_config(), Box::new(|_| {}))
            .expect("start");
        // Sent while a session is active: must not be dropped, only deferred.
        actor.set_unload_timeout_for_test(Duration::from_millis(50));
        audio_tx.send(end_of_stream(1)).unwrap();
        actor.stop_session(Duration::from_secs(2)).expect("stop");

        assert!(
            wait_for(|| unload_count.load(Ordering::SeqCst) >= 1, Duration::from_secs(2)),
            "timeout set mid-session must still apply once the session ends"
        );
    }

    #[test]
    fn unload_timeout_disabled_by_default_never_unloads() {
        let mock = MockEngine::new();
        let unload_count = mock.unload_count_handle();
        let (actor, _audio_tx) = spawn_with_mock(mock);

        // 0 minutes == disabled; explicit, matching the default setting.
        actor.set_unload_timeout(0);

        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(
            unload_count.load(Ordering::SeqCst),
            0,
            "a disabled (0 minute) timeout must never unload the model"
        );
    }

    // Idle-monitor wiring: the monitor's own behavior (signal cadence, re-arm,
    // max duration) is covered exhaustively in `pipeline::idle`'s unit tests,
    // which run on bare `Instant`s with no thread involved. These tests only
    // verify the actor accepts `idle_config` and runs a session to completion
    // without panicking. There is no `AppHandle` in these tests (`AttachApp`
    // is never sent), so the event-emission path can't be observed here.
    #[test]
    fn session_with_idle_config_runs_to_completion() {
        let mock = MockEngine::new().with_transcribe_response(Ok(vec![seg("hello")]), 3);
        let (actor, audio_tx) = spawn_with_mock(mock);

        let cfg = SessionConfig {
            idle_config: Some(super::MeetingIdleConfig {
                silence_threshold: Some(Duration::from_millis(50)),
                max_duration: None,
            }),
            ..session_config()
        };
        let (collected, cb) = collecting_callback();
        actor.start_session(1, cfg, cb).expect("start with idle config");

        // Give the actor loop a few ticks past the silence threshold with no
        // segments arriving, then confirm it is still alive and stops cleanly.
        std::thread::sleep(Duration::from_millis(120));
        for _ in 0..3 {
            audio_tx.send(audio_chunk(1)).unwrap();
        }
        audio_tx.send(end_of_stream(1)).unwrap();

        actor
            .stop_session(Duration::from_secs(2))
            .expect("stop session with idle config");
        assert!(
            collected.lock().unwrap().iter().any(|s| s.text == "hello"),
            "session should keep transcribing normally alongside idle tracking"
        );
    }

    #[test]
    fn session_with_idle_config_and_active_speech_never_stalls() {
        // A tiny max_duration alongside a segment stream: the actor must keep
        // draining audio and stop normally even once max-duration has crossed.
        let mock = MockEngine::new().with_transcribe_response(Ok(vec![seg("hi")]), 5);
        let (actor, audio_tx) = spawn_with_mock(mock);

        let cfg = SessionConfig {
            idle_config: Some(super::MeetingIdleConfig {
                silence_threshold: None,
                max_duration: Some(Duration::from_millis(10)),
            }),
            ..session_config()
        };
        let (collected, cb) = collecting_callback();
        actor.start_session(1, cfg, cb).expect("start with idle config");

        std::thread::sleep(Duration::from_millis(50));
        for _ in 0..5 {
            audio_tx.send(audio_chunk(1)).unwrap();
        }
        audio_tx.send(end_of_stream(1)).unwrap();

        let summary = actor
            .stop_session(Duration::from_secs(2))
            .expect("stop session with max-duration idle config");
        assert_eq!(summary.frames_processed, 5);
        assert!(collected.lock().unwrap().iter().any(|s| s.text == "hi"));
    }

    /// Test-only VAD stand-in whose speech/silence decision is toggled from
    /// outside, so the drain-window logic can be exercised deterministically
    /// without a real Silero model.
    struct ToggleVad(Arc<std::sync::atomic::AtomicBool>);

    impl crate::filter::AudioFilter for ToggleVad {
        fn kind(&self) -> crate::filter::AudioFilterKind {
            crate::filter::AudioFilterKind::SileroVad
        }
        fn process(&mut self, _audio: &[f32]) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        fn reset(&mut self) {}
    }

    #[test]
    fn single_mode_drain_window_feeds_engine_then_gates() {
        use std::sync::atomic::AtomicBool;

        let chunk_size = MIMI_FRAME_SIZE;
        let make_frame = || AudioChunk {
            session_id: 1,
            samples: vec![0.0f32; chunk_size],
            captured_at: Instant::now(),
            speaker: None,
        };

        let speech = Arc::new(AtomicBool::new(true));
        let chain =
            crate::filter::AudioFilterChain::new(vec![Box::new(ToggleVad(Arc::clone(&speech)))]);
        let drain_window = 3;
        let mut mode = SingleMode::new(chain, drain_window);
        let mut engine = MockEngine::new();

        // Speech: fed and counted as transcribed, not skipped.
        mode.ingest(make_frame());
        assert!(matches!(mode.step(&mut engine, chunk_size), Ok(Some(_))));

        // Speech stops: the drain window still feeds the next `drain_window`
        // frames to the engine so the emission-delayed tail word can surface.
        speech.store(false, Ordering::SeqCst);
        for i in 0..drain_window {
            mode.ingest(make_frame());
            let result = mode.step(&mut engine, chunk_size);
            assert!(
                matches!(result, Ok(Some(_))),
                "frame {i} within the drain window should still reach the engine"
            );
        }
        assert_eq!(mode.vad_drained, drain_window as u64);
        assert_eq!(mode.vad_skipped, 0);

        // Beyond the window: now genuinely gated.
        mode.ingest(make_frame());
        let result = mode.step(&mut engine, chunk_size);
        assert!(
            matches!(result, Ok(None)),
            "frame past the drain window should be VAD-gated"
        );
        assert_eq!(mode.vad_skipped, 1);

        // Speech resumes: the drain-window counter resets.
        speech.store(true, Ordering::SeqCst);
        mode.ingest(make_frame());
        assert!(matches!(mode.step(&mut engine, chunk_size), Ok(Some(_))));
    }

    #[test]
    fn start_session_skips_reset_when_prewarmed_and_resets_on_mode_change() {
        let mock = MockEngine::new();
        let reset_count = mock.reset_state_count_handle();
        let (actor, audio_tx) = spawn_with_mock(mock);

        // load_model alone builds fresh single-stream state; no reset needed yet.
        assert_eq!(reset_count.load(Ordering::SeqCst), 0);

        actor
            .start_session(1, session_config(), Box::new(|_| {}))
            .expect("start dictation 1");
        assert_eq!(
            reset_count.load(Ordering::SeqCst),
            0,
            "pre-warmed single-stream state must skip reset_state on start"
        );
        audio_tx.send(end_of_stream(1)).unwrap();
        actor.stop_session(Duration::from_secs(2)).expect("stop 1");

        // The idle pre-warm runs on the actor thread AFTER the stop reply
        // (so it never adds to stop latency), so poll for it instead of
        // asserting immediately.
        assert!(
            wait_for(|| reset_count.load(Ordering::SeqCst) >= 1, Duration::from_secs(2)),
            "session end should pre-warm single-stream state"
        );

        actor
            .start_session(2, session_config(), Box::new(|_| {}))
            .expect("start dictation 2");
        assert_eq!(
            reset_count.load(Ordering::SeqCst),
            1,
            "second dictation start should reuse the pre-warmed state"
        );
        audio_tx.send(end_of_stream(2)).unwrap();
        actor.stop_session(Duration::from_secs(2)).expect("stop 2");
        assert!(
            wait_for(|| reset_count.load(Ordering::SeqCst) >= 2, Duration::from_secs(2)),
            "second session end should pre-warm again"
        );

        // A diarized start after a single-stream pre-warm must still pay the reset.
        let diarized_cfg = SessionConfig {
            diarize: true,
            ..session_config()
        };
        actor
            .start_session(3, diarized_cfg, Box::new(|_| {}))
            .expect("start diarized");
        assert_eq!(
            reset_count.load(Ordering::SeqCst),
            3,
            "diarize mode change must pay the reset even when pre-warmed"
        );
        audio_tx.send(end_of_stream(3)).unwrap();
        actor
            .stop_session(Duration::from_secs(2))
            .expect("stop diarized");
        assert!(
            wait_for(|| reset_count.load(Ordering::SeqCst) >= 4, Duration::from_secs(2)),
            "post-session pre-warm resets back to single-stream"
        );
    }
}
