use std::time::Duration;

use crossbeam_channel::Receiver;
use tracing::{debug, error, info, warn};

use crate::audio::AudioChunk;
use crate::constants::MIMI_FRAME_SIZE;
use crate::engine::{SharedTranscriptionEngine, TranscriptionEngine, TranscriptionSegment};
use crate::platform::with_autorelease_pool;

/// Callback type for streaming segments to the frontend
pub type SegmentCallback = Box<dyn Fn(TranscriptionSegment) + Send + 'static>;

/// Commands sent to the inference thread
pub enum InferenceCommand {
    /// Start a new transcription session with the given segment callback
    Start {
        session_id: u64,
        on_segment: SegmentCallback,
    },
    /// Stop transcribing, flush remaining audio, signal completion via sender
    Stop(crossbeam_channel::Sender<()>),
    /// Shutdown the inference thread entirely
    Shutdown,
}

/// Persistent transcription pipeline running on a dedicated std::thread.
/// Spawned once at model load time. Accepts Start/Stop commands for each
/// recording session — no thread creation overhead per recording.
pub struct TranscriptionPipeline {
    cmd_sender: crossbeam_channel::Sender<InferenceCommand>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl TranscriptionPipeline {
    /// Spawn the persistent inference thread. The thread stays alive across
    /// recording sessions, waiting for Start/Stop commands.
    pub fn spawn(
        audio_rx: Receiver<AudioChunk>,
        engine: SharedTranscriptionEngine,
    ) -> Result<Self, String> {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<InferenceCommand>();

        let handle = std::thread::Builder::new()
            .name("inference".into())
            .spawn(move || {
                Self::inference_loop(cmd_rx, audio_rx, engine);
            })
            .map_err(|e| format!("Failed to spawn inference thread: {e}"))?;

        Ok(Self {
            cmd_sender: cmd_tx,
            handle: Some(handle),
        })
    }

    pub fn send(&self, cmd: InferenceCommand) -> Result<(), String> {
        self.cmd_sender
            .send(cmd)
            .map_err(|e| format!("Inference thread disconnected: {e}"))
    }

    pub fn shutdown(&mut self) -> Result<(), String> {
        if let Some(handle) = self.handle.take() {
            let _ = self.cmd_sender.send(InferenceCommand::Shutdown);
            handle
                .join()
                .map_err(|_| "Inference thread panicked during shutdown".to_string())?;
        }
        Ok(())
    }

    fn inference_loop(
        cmd_rx: crossbeam_channel::Receiver<InferenceCommand>,
        audio_rx: Receiver<AudioChunk>,
        engine: SharedTranscriptionEngine,
    ) {
        info!("Inference thread started (persistent)");
        let mut session_count: u32 = 0;

        loop {
            // Wait for next command
            match cmd_rx.recv() {
                Ok(InferenceCommand::Start {
                    session_id,
                    on_segment,
                }) => {
                    session_count += 1;
                    info!(
                        "Inference session {session_count} starting (audio session {session_id})"
                    );

                    // Hold engine lock for the entire active session.
                    // State reset is done by the caller before sending Start,
                    // so teardown/rebuild never overlaps active inference.
                    let guard = match engine.lock() {
                        Ok(g) => g,
                        Err(e) => {
                            error!("Engine lock failed: {e}");
                            continue;
                        }
                    };

                    // Wrap entire session in autorelease pool. Per-frame pools
                    // in transcribe() drain most Metal objects, but this session
                    // pool catches anything that escapes (the inference thread
                    // has no top-level ObjC autorelease pool).
                    let normal_stop = with_autorelease_pool(|| {
                        Self::active_loop(
                            &cmd_rx,
                            &audio_rx,
                            guard.as_ref(),
                            &on_segment,
                            session_id,
                        )
                    });
                    drop(guard);

                    if !normal_stop {
                        info!("Inference thread shutting down");
                        return;
                    }
                    info!("Inference session {session_count} ended");
                }
                Ok(InferenceCommand::Stop(_)) => {} // already idle, ignore
                Ok(InferenceCommand::Shutdown) => {
                    info!("Inference thread shutting down");
                    return;
                }
                Err(_) => {
                    warn!("Command channel disconnected, exiting");
                    return;
                }
            }
        }
    }

    /// Process audio frames while the session is active.
    /// Returns true for normal stop, false for shutdown/disconnect.
    fn active_loop(
        cmd_rx: &crossbeam_channel::Receiver<InferenceCommand>,
        audio_rx: &Receiver<AudioChunk>,
        engine: &dyn TranscriptionEngine,
        on_segment: &SegmentCallback,
        session_id: u64,
    ) -> bool {
        let mut audio_buffer: Vec<f32> = Vec::new();
        let mut frames_processed: u64 = 0;
        let mut skipped_chunks: u64 = 0;

        loop {
            // Check for stop/shutdown command (non-blocking)
            match cmd_rx.try_recv() {
                Ok(InferenceCommand::Stop(done_tx)) => {
                    if crate::debug::transcription_debug_enabled() {
                        debug!("Stopping ({frames_processed} frames processed)");
                    }

                    // Drain ALL remaining audio from the channel.
                    // Audio capture was stopped before this command was sent,
                    // so no new chunks will arrive — we just collect what's left.
                    let mut drained = 0usize;
                    while let Ok(chunk) = audio_rx.try_recv() {
                        if chunk.session_id == session_id {
                            audio_buffer.extend_from_slice(&chunk.samples);
                            drained += 1;
                        } else {
                            skipped_chunks += 1;
                        }
                    }
                    if drained > 0 && crate::debug::transcription_debug_enabled() {
                        debug!("Drained {drained} remaining audio chunks on stop");
                    }
                    if skipped_chunks > 0 && crate::debug::transcription_debug_enabled() {
                        debug!("Ignored {skipped_chunks} stale audio chunks during stop");
                    }

                    // Process all buffered audio through the engine (frame by frame)
                    while audio_buffer.len() >= MIMI_FRAME_SIZE {
                        let frame: Vec<f32> = audio_buffer.drain(..MIMI_FRAME_SIZE).collect();
                        match engine.transcribe(&frame, None) {
                            Ok(segments) => {
                                for seg in &segments {
                                    if crate::debug::transcription_debug_enabled() {
                                        debug!(
                                            "Drain segment: {:?} final={}",
                                            seg.text, seg.is_final
                                        );
                                    }
                                    on_segment(seg.clone());
                                }
                            }
                            Err(e) => {
                                error!("Transcribe error during drain: {e}");
                                break;
                            }
                        }
                    }
                    // Process any remaining partial frame
                    if !audio_buffer.is_empty() {
                        Self::process_frames(engine, &audio_buffer, on_segment);
                        audio_buffer.clear();
                    }

                    // Flush engine (feeds silence to extract remaining buffered tokens)
                    match engine.flush() {
                        Ok(segments) => {
                            if crate::debug::transcription_debug_enabled() {
                                debug!("Flush: {} segments", segments.len());
                            }
                            for seg in segments {
                                on_segment(seg);
                            }
                        }
                        Err(e) => error!("Flush error: {e}"),
                    }

                    // Signal caller that drain/flush is complete
                    let _ = done_tx.send(());
                    return true;
                }
                Ok(InferenceCommand::Shutdown) => return false,
                _ => {}
            }

            // Read audio with short timeout
            match audio_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(chunk) => {
                    if chunk.session_id != session_id {
                        skipped_chunks += 1;
                        if crate::debug::transcription_debug_enabled()
                            && (skipped_chunks <= 5 || skipped_chunks.is_multiple_of(25))
                        {
                            debug!(
                                "Ignoring stale audio chunk from session {} while expecting {}",
                                chunk.session_id, session_id
                            );
                        }
                        continue;
                    }

                    audio_buffer.extend_from_slice(&chunk.samples);

                    // Process complete 1920-sample frames
                    while audio_buffer.len() >= MIMI_FRAME_SIZE {
                        let frame: Vec<f32> = audio_buffer.drain(..MIMI_FRAME_SIZE).collect();
                        match engine.transcribe(&frame, None) {
                            Ok(segments) => {
                                for seg in &segments {
                                    if crate::debug::transcription_debug_enabled() {
                                        debug!("Segment: {:?} final={}", seg.text, seg.is_final);
                                    }
                                    on_segment(seg.clone());
                                }
                            }
                            Err(e) => {
                                error!("Transcribe error: {e}");
                                return false;
                            }
                        }
                        frames_processed += 1;
                        if crate::debug::transcription_debug_enabled()
                            && frames_processed.is_multiple_of(50)
                        {
                            debug!(
                                "Processed {frames_processed} frames ({:.1}s)",
                                frames_processed as f64 * 0.08,
                            );
                        }
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    warn!("Audio channel disconnected");
                    return false;
                }
            }
        }
    }

    fn process_frames(
        engine: &dyn TranscriptionEngine,
        audio: &[f32],
        on_segment: &SegmentCallback,
    ) {
        match engine.transcribe(audio, None) {
            Ok(segments) => {
                for seg in segments {
                    on_segment(seg);
                }
            }
            Err(e) => error!("Transcribe error: {e}"),
        }
    }
}

impl Drop for TranscriptionPipeline {
    fn drop(&mut self) {
        if let Err(e) = self.shutdown() {
            warn!("Inference pipeline drop failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crossbeam_channel::unbounded;

    use crate::engine::default_transcription_engine;

    use super::TranscriptionPipeline;

    #[test]
    fn pipeline_shutdown_is_idempotent() {
        let (_audio_tx, audio_rx) = unbounded();
        let engine = Arc::new(Mutex::new(default_transcription_engine()));
        let mut pipeline = TranscriptionPipeline::spawn(audio_rx, engine).expect("spawn pipeline");

        pipeline.shutdown().expect("first shutdown");
        pipeline.shutdown().expect("second shutdown");
    }
}
