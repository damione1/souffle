use std::time::Duration;

use crossbeam_channel::Receiver;
use tracing::{debug, error, info, warn};

use crate::audio::AudioChunk;
use crate::engine::{SharedTranscriptionEngine, TranscriptionEngine, TranscriptionSegment};
use crate::filter::{
    AudioFilterChain, DictionaryEntry, PipelineConfig, TextFilterChain, build_audio_filters,
    build_text_filters,
};
use crate::platform::with_autorelease_pool;

/// Callback type for streaming segments to the frontend
pub type SegmentCallback = Box<dyn Fn(TranscriptionSegment) + Send + 'static>;

/// Commands sent to the inference thread
pub enum InferenceCommand {
    /// Start a new transcription session with the given segment callback.
    /// Filter chains are built ON the inference thread to avoid Metal/ONNX
    /// conflicts between ort (Silero VAD) and whisper.cpp on the main thread.
    Start {
        session_id: u64,
        on_segment: SegmentCallback,
        pipeline_config: PipelineConfig,
        source_sample_rate: u32,
        dictionary_entries: Vec<DictionaryEntry>,
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
                    pipeline_config,
                    source_sample_rate,
                    dictionary_entries,
                }) => {
                    session_count += 1;
                    info!(
                        "Inference session {session_count} starting (audio session {session_id})"
                    );

                    // Build filter chains ON the inference thread to keep ONNX
                    // Runtime (Silero VAD) init on the same thread as whisper.cpp
                    // Metal work, avoiding Metal residency set conflicts.
                    let mut audio_filters =
                        build_audio_filters(&pipeline_config, source_sample_rate);
                    let text_filters =
                        build_text_filters(&pipeline_config, dictionary_entries);

                    // Hold engine lock for the entire active session.
                    // State reset is done by the caller before sending Start,
                    // so teardown/rebuild never overlaps active inference.
                    let mut guard = match engine.lock() {
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
                            guard.as_mut(),
                            &on_segment,
                            session_id,
                            &mut audio_filters,
                            &text_filters,
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
        engine: &mut dyn TranscriptionEngine,
        on_segment: &SegmentCallback,
        session_id: u64,
        audio_filters: &mut AudioFilterChain,
        text_filters: &TextFilterChain,
    ) -> bool {
        let chunk_size = engine.audio_requirements().chunk_size_samples as usize;
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
                    while audio_buffer.len() >= chunk_size {
                        let frame: Vec<f32> = audio_buffer.drain(..chunk_size).collect();
                        match engine.transcribe(&frame, None) {
                            Ok(segments) => {
                                for seg in &segments {
                                    if crate::debug::transcription_debug_enabled() {
                                        debug!(
                                            "Drain segment: {:?} final={}",
                                            seg.text, seg.is_final
                                        );
                                    }
                                    Self::emit_filtered(engine, text_filters, seg.clone(), on_segment);
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
                        Self::process_frames(engine, text_filters, &audio_buffer, on_segment);
                        audio_buffer.clear();
                    }

                    // Flush engine (feeds silence to extract remaining buffered tokens)
                    match engine.flush() {
                        Ok(segments) => {
                            if crate::debug::transcription_debug_enabled() {
                                debug!("Flush: {} segments", segments.len());
                            }
                            for seg in segments {
                                Self::emit_filtered(engine, text_filters, seg, on_segment);
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

                    // Process complete engine-sized frames
                    while audio_buffer.len() >= chunk_size {
                        let frame: Vec<f32> = audio_buffer.drain(..chunk_size).collect();
                        if !audio_filters.process(&frame) {
                            frames_processed += 1;
                            continue; // VAD says no speech — skip this frame
                        }
                        match engine.transcribe(&frame, None) {
                            Ok(segments) => {
                                for seg in &segments {
                                    if crate::debug::transcription_debug_enabled() {
                                        debug!("Segment: {:?} final={}", seg.text, seg.is_final);
                                    }
                                    Self::emit_filtered(engine, text_filters, seg.clone(), on_segment);
                                }
                            }
                            Err(e) => {
                                // Do not tear down the inference thread — log and skip this frame.
                                error!("Transcribe error (frame skipped): {e}");
                            }
                        }
                        frames_processed += 1;
                        if crate::debug::transcription_debug_enabled()
                            && frames_processed.is_multiple_of(50)
                        {
                            let frame_duration = chunk_size as f64
                                / engine.audio_requirements().sample_rate_hz as f64;
                            debug!(
                                "Processed {frames_processed} frames ({:.1}s)",
                                frames_processed as f64 * frame_duration,
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
        engine: &mut dyn TranscriptionEngine,
        text_filters: &TextFilterChain,
        audio: &[f32],
        on_segment: &SegmentCallback,
    ) {
        match engine.transcribe(audio, None) {
            Ok(segments) => {
                for seg in segments {
                    Self::emit_filtered(engine, text_filters, seg, on_segment);
                }
            }
            Err(e) => error!("Transcribe error: {e}"),
        }
    }

    /// Normalize engine-specific tokens, then apply text filter chain before emitting.
    fn emit_filtered(
        engine: &dyn TranscriptionEngine,
        text_filters: &TextFilterChain,
        mut segment: crate::engine::TranscriptionSegment,
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
    use std::time::Duration;

    use crossbeam_channel::unbounded;

    use crate::audio::AudioChunk;
    use crate::constants::MIMI_FRAME_SIZE;
    use crate::engine::default_transcription_engine;
    use crate::engine::mock::MockEngine;
    use crate::engine::TranscriptionSegment;

    use crate::filter::PipelineConfig;

    use super::{InferenceCommand, TranscriptionPipeline};

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

    /// Helper: create a TranscriptionSegment with the given text.
    fn seg(text: &str) -> TranscriptionSegment {
        TranscriptionSegment {
            text: text.to_string(),
            start_time: 0.0,
            end_time: 0.0,
            is_final: false,
            language: None,
            confidence: None,
        }
    }

    /// Helper: create an AudioChunk with MIMI_FRAME_SIZE samples for the given session.
    fn audio_chunk(session_id: u64) -> AudioChunk {
        AudioChunk {
            session_id,
            samples: vec![0.0f32; MIMI_FRAME_SIZE],
        }
    }

    #[test]
    fn pipeline_shutdown_is_idempotent() {
        let (_audio_tx, audio_rx) = unbounded();
        let engine = Arc::new(Mutex::new(default_transcription_engine()));
        let mut pipeline = TranscriptionPipeline::spawn(audio_rx, engine).expect("spawn pipeline");

        pipeline.shutdown().expect("first shutdown");
        pipeline.shutdown().expect("second shutdown");
    }

    #[test]
    fn pipeline_processes_audio_frames() {
        let (audio_tx, audio_rx) = unbounded();

        let mock = MockEngine::new().with_transcribe_response(Ok(vec![seg("hello")]), 3);
        let engine: Arc<Mutex<Box<dyn crate::engine::TranscriptionEngine>>> =
            Arc::new(Mutex::new(Box::new(mock)));

        let mut pipeline = TranscriptionPipeline::spawn(audio_rx, engine).expect("spawn");

        let collected: Arc<Mutex<Vec<TranscriptionSegment>>> = Arc::new(Mutex::new(Vec::new()));
        let collected_cb = Arc::clone(&collected);

        pipeline
            .send(InferenceCommand::Start {
                session_id: 1,
                on_segment: Box::new(move |s| {
                    collected_cb.lock().unwrap().push(s);
                }),
                pipeline_config: noop_filter_config(),
                source_sample_rate: 16000,
                dictionary_entries: vec![],
            })
            .expect("start");

        // Send 3 frame-sized chunks so engine produces segments
        for _ in 0..3 {
            audio_tx.send(audio_chunk(1)).unwrap();
        }

        // Allow processing time
        std::thread::sleep(Duration::from_millis(300));

        let (done_tx, done_rx) = unbounded();
        pipeline
            .send(InferenceCommand::Stop(done_tx))
            .expect("stop");
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("done signal");

        let segments = collected.lock().unwrap();
        assert!(
            segments.iter().any(|s| s.text == "hello"),
            "Expected at least one 'hello' segment, got: {:?}",
            segments.iter().map(|s| &s.text).collect::<Vec<_>>()
        );

        pipeline.shutdown().expect("shutdown");
    }

    #[test]
    fn pipeline_start_stop_cycle() {
        let (audio_tx, audio_rx) = unbounded();

        let mock = MockEngine::new();
        let engine: Arc<Mutex<Box<dyn crate::engine::TranscriptionEngine>>> =
            Arc::new(Mutex::new(Box::new(mock)));

        let mut pipeline = TranscriptionPipeline::spawn(audio_rx, engine).expect("spawn");

        pipeline
            .send(InferenceCommand::Start {
                session_id: 1,
                on_segment: Box::new(|_| {}),
                pipeline_config: noop_filter_config(),
                source_sample_rate: 16000,
                dictionary_entries: vec![],
            })
            .expect("start");

        audio_tx.send(audio_chunk(1)).unwrap();
        std::thread::sleep(Duration::from_millis(100));

        let (done_tx, done_rx) = unbounded();
        pipeline
            .send(InferenceCommand::Stop(done_tx))
            .expect("stop");
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("should receive done signal after stop");

        pipeline.shutdown().expect("shutdown");
    }

    #[test]
    fn pipeline_flush_on_stop() {
        let (_audio_tx, audio_rx) = unbounded();

        let mock = MockEngine::new().with_flush_response(Ok(vec![seg("flushed")]));
        let engine: Arc<Mutex<Box<dyn crate::engine::TranscriptionEngine>>> =
            Arc::new(Mutex::new(Box::new(mock)));

        let mut pipeline = TranscriptionPipeline::spawn(audio_rx, engine).expect("spawn");

        let collected: Arc<Mutex<Vec<TranscriptionSegment>>> = Arc::new(Mutex::new(Vec::new()));
        let collected_cb = Arc::clone(&collected);

        pipeline
            .send(InferenceCommand::Start {
                session_id: 1,
                on_segment: Box::new(move |s| {
                    collected_cb.lock().unwrap().push(s);
                }),
                pipeline_config: noop_filter_config(),
                source_sample_rate: 16000,
                dictionary_entries: vec![],
            })
            .expect("start");

        // Small delay so the active_loop is running
        std::thread::sleep(Duration::from_millis(100));

        let (done_tx, done_rx) = unbounded();
        pipeline
            .send(InferenceCommand::Stop(done_tx))
            .expect("stop");
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("done signal");

        let segments = collected.lock().unwrap();
        assert!(
            segments.iter().any(|s| s.text == "flushed"),
            "Expected flush segment 'flushed', got: {:?}",
            segments.iter().map(|s| &s.text).collect::<Vec<_>>()
        );

        pipeline.shutdown().expect("shutdown");
    }

    #[test]
    fn pipeline_drain_remaining_on_stop() {
        let (audio_tx, audio_rx) = unbounded();

        // Provide enough transcribe responses for all chunks that will be drained
        let mock = MockEngine::new().with_transcribe_response(Ok(vec![seg("drained")]), 5);
        let engine: Arc<Mutex<Box<dyn crate::engine::TranscriptionEngine>>> =
            Arc::new(Mutex::new(Box::new(mock)));

        let mut pipeline = TranscriptionPipeline::spawn(audio_rx, engine).expect("spawn");

        let collected: Arc<Mutex<Vec<TranscriptionSegment>>> = Arc::new(Mutex::new(Vec::new()));
        let collected_cb = Arc::clone(&collected);

        pipeline
            .send(InferenceCommand::Start {
                session_id: 1,
                on_segment: Box::new(move |s| {
                    collected_cb.lock().unwrap().push(s);
                }),
                pipeline_config: noop_filter_config(),
                source_sample_rate: 16000,
                dictionary_entries: vec![],
            })
            .expect("start");

        // Send multiple chunks quickly, then immediately stop.
        // Some will be processed in the active loop, the rest should be
        // drained during the Stop handling.
        for _ in 0..5 {
            audio_tx.send(audio_chunk(1)).unwrap();
        }

        let (done_tx, done_rx) = unbounded();
        pipeline
            .send(InferenceCommand::Stop(done_tx))
            .expect("stop");
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("done signal");

        let segments = collected.lock().unwrap();
        let drained_count = segments.iter().filter(|s| s.text == "drained").count();
        assert!(
            drained_count >= 1,
            "Expected at least 1 'drained' segment from queued audio, got {drained_count}"
        );

        pipeline.shutdown().expect("shutdown");
    }

    #[test]
    fn pipeline_multiple_sessions() {
        let (audio_tx, audio_rx) = unbounded();

        // Provide responses for 2 sessions (transcribe + flush each)
        let mock = MockEngine::new()
            .with_transcribe_response(Ok(vec![seg("s1")]), 2)
            .with_transcribe_response(Ok(vec![seg("s2")]), 2)
            .with_flush_response(Ok(vec![]))
            .with_flush_response(Ok(vec![]));
        let engine: Arc<Mutex<Box<dyn crate::engine::TranscriptionEngine>>> =
            Arc::new(Mutex::new(Box::new(mock)));

        let mut pipeline = TranscriptionPipeline::spawn(audio_rx, engine).expect("spawn");

        // --- Session 1 ---
        let collected1: Arc<Mutex<Vec<TranscriptionSegment>>> = Arc::new(Mutex::new(Vec::new()));
        let cb1 = Arc::clone(&collected1);
        pipeline
            .send(InferenceCommand::Start {
                session_id: 1,
                on_segment: Box::new(move |s| {
                    cb1.lock().unwrap().push(s);
                }),
                pipeline_config: noop_filter_config(),
                source_sample_rate: 16000,
                dictionary_entries: vec![],
            })
            .expect("start session 1");

        audio_tx.send(audio_chunk(1)).unwrap();
        std::thread::sleep(Duration::from_millis(200));

        let (done_tx1, done_rx1) = unbounded();
        pipeline
            .send(InferenceCommand::Stop(done_tx1))
            .expect("stop session 1");
        done_rx1
            .recv_timeout(Duration::from_secs(2))
            .expect("done signal session 1");

        // --- Session 2 ---
        let collected2: Arc<Mutex<Vec<TranscriptionSegment>>> = Arc::new(Mutex::new(Vec::new()));
        let cb2 = Arc::clone(&collected2);
        pipeline
            .send(InferenceCommand::Start {
                session_id: 2,
                on_segment: Box::new(move |s| {
                    cb2.lock().unwrap().push(s);
                }),
                pipeline_config: noop_filter_config(),
                source_sample_rate: 16000,
                dictionary_entries: vec![],
            })
            .expect("start session 2");

        audio_tx.send(audio_chunk(2)).unwrap();
        std::thread::sleep(Duration::from_millis(200));

        let (done_tx2, done_rx2) = unbounded();
        pipeline
            .send(InferenceCommand::Stop(done_tx2))
            .expect("stop session 2");
        done_rx2
            .recv_timeout(Duration::from_secs(2))
            .expect("done signal session 2");

        let segs1 = collected1.lock().unwrap();
        let segs2 = collected2.lock().unwrap();
        assert!(
            !segs1.is_empty(),
            "Session 1 should have produced segments"
        );
        assert!(
            !segs2.is_empty(),
            "Session 2 should have produced segments"
        );

        pipeline.shutdown().expect("shutdown");
    }

    #[test]
    fn pipeline_ignores_stale_session() {
        let (audio_tx, audio_rx) = unbounded();

        // Only provide transcribe responses for session 2 audio
        let mock = MockEngine::new()
            .with_transcribe_response(Ok(vec![seg("fresh")]), 3)
            .with_flush_response(Ok(vec![]));
        let engine: Arc<Mutex<Box<dyn crate::engine::TranscriptionEngine>>> =
            Arc::new(Mutex::new(Box::new(mock)));

        let mut pipeline = TranscriptionPipeline::spawn(audio_rx, engine).expect("spawn");

        // Start session 2, but send audio with session_id=1 (stale)
        let collected: Arc<Mutex<Vec<TranscriptionSegment>>> = Arc::new(Mutex::new(Vec::new()));
        let collected_cb = Arc::clone(&collected);

        pipeline
            .send(InferenceCommand::Start {
                session_id: 2,
                on_segment: Box::new(move |s| {
                    collected_cb.lock().unwrap().push(s);
                }),
                pipeline_config: noop_filter_config(),
                source_sample_rate: 16000,
                dictionary_entries: vec![],
            })
            .expect("start");

        // Send stale chunks (session_id=1, but active session is 2)
        for _ in 0..3 {
            audio_tx
                .send(AudioChunk {
                    session_id: 1,
                    samples: vec![0.0f32; MIMI_FRAME_SIZE],
                })
                .unwrap();
        }
        std::thread::sleep(Duration::from_millis(200));

        // Now send one valid chunk (session_id=2)
        audio_tx.send(audio_chunk(2)).unwrap();
        std::thread::sleep(Duration::from_millis(200));

        let (done_tx, done_rx) = unbounded();
        pipeline
            .send(InferenceCommand::Stop(done_tx))
            .expect("stop");
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("done signal");

        let segments = collected.lock().unwrap();
        // Stale chunks should have been ignored; only the session-2 chunk should
        // have triggered transcription, so we expect exactly 1 "fresh" segment.
        let fresh_count = segments.iter().filter(|s| s.text == "fresh").count();
        assert_eq!(
            fresh_count, 1,
            "Expected exactly 1 segment from session-2 audio, got {fresh_count} (stale audio should be ignored)"
        );

        pipeline.shutdown().expect("shutdown");
    }
}
