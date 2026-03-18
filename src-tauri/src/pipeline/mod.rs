use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::Receiver;

use crate::engine::kyutai::KyutaiEngine;
use crate::engine::{TranscriptionEngine, TranscriptionSegment};

/// Commands sent to the inference thread
pub enum InferenceCommand {
    /// Start transcribing audio from the receiver
    Start,
    /// Stop transcribing, flush remaining audio
    Stop,
    /// Shutdown the inference thread entirely
    Shutdown,
}

/// Callback type for streaming segments to the frontend
pub type SegmentCallback = Box<dyn Fn(TranscriptionSegment) + Send + 'static>;

/// The transcription pipeline runs on a dedicated std::thread.
/// It reads audio chunks from the crossbeam channel, feeds them to the engine,
/// and sends transcription segments back via a callback.
pub struct TranscriptionPipeline {
    cmd_sender: crossbeam_channel::Sender<InferenceCommand>,
}

impl TranscriptionPipeline {
    /// Spawn the inference thread. Returns a handle for sending commands.
    pub fn spawn(
        audio_rx: Receiver<Vec<f32>>,
        engine: Arc<Mutex<KyutaiEngine>>,
        on_segment: SegmentCallback,
    ) -> Self {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<InferenceCommand>();

        std::thread::Builder::new()
            .name("inference".into())
            .spawn(move || {
                Self::inference_loop(cmd_rx, audio_rx, engine, on_segment);
            })
            .expect("Failed to spawn inference thread");

        Self { cmd_sender: cmd_tx }
    }

    pub fn send(&self, cmd: InferenceCommand) -> Result<(), String> {
        self.cmd_sender
            .send(cmd)
            .map_err(|e| format!("Inference thread disconnected: {e}"))
    }

    fn inference_loop(
        cmd_rx: crossbeam_channel::Receiver<InferenceCommand>,
        audio_rx: Receiver<Vec<f32>>,
        engine: Arc<Mutex<KyutaiEngine>>,
        on_segment: SegmentCallback,
    ) {
        eprintln!("[souffle] Inference thread started");

        loop {
            // Wait for Start command
            match cmd_rx.recv() {
                Ok(InferenceCommand::Start) => {
                    eprintln!("[souffle] Inference starting");
                    // Drain stale audio
                    let drained = std::iter::from_fn(|| audio_rx.try_recv().ok()).count();
                    if drained > 0 {
                        eprintln!("[souffle] Drained {drained} stale audio chunks");
                    }

                    // Hold engine lock for the entire active session
                    // This matches the batch behavior where all frames are
                    // processed under one continuous lock.
                    let guard = match engine.lock() {
                        Ok(g) => g,
                        Err(e) => {
                            eprintln!("[souffle] Engine lock failed: {e}");
                            continue;
                        }
                    };

                    Self::active_loop(&cmd_rx, &audio_rx, &guard, &on_segment);

                    // Lock is dropped here when active_loop returns
                    eprintln!("[souffle] Inference session ended");
                }
                Ok(InferenceCommand::Shutdown) => {
                    eprintln!("[souffle] Inference thread shutting down");
                    return;
                }
                Ok(InferenceCommand::Stop) => {} // already idle
                Err(_) => {
                    eprintln!("[souffle] Command channel disconnected, exiting");
                    return;
                }
            }
        }
    }

    /// Process audio frames while the session is active.
    /// The engine lock is held for the entire duration.
    fn active_loop(
        cmd_rx: &crossbeam_channel::Receiver<InferenceCommand>,
        audio_rx: &Receiver<Vec<f32>>,
        engine: &KyutaiEngine,
        on_segment: &SegmentCallback,
    ) {
        let mut audio_buffer: Vec<f32> = Vec::new();
        let mut frames_processed: u64 = 0;

        loop {
            // Check for stop command (non-blocking)
            match cmd_rx.try_recv() {
                Ok(InferenceCommand::Stop) => {
                    eprintln!("[souffle] Stopping ({frames_processed} frames processed)");

                    // Drain ALL remaining audio from the channel.
                    // Audio capture was stopped before this command was sent,
                    // so no new chunks will arrive — we just collect what's left.
                    let mut drained = 0usize;
                    loop {
                        match audio_rx.try_recv() {
                            Ok(chunk) => {
                                audio_buffer.extend_from_slice(&chunk);
                                drained += 1;
                            }
                            Err(_) => break,
                        }
                    }
                    if drained > 0 {
                        eprintln!("[souffle] Drained {drained} remaining audio chunks on stop");
                    }

                    // Process all buffered audio through the engine (frame by frame)
                    while audio_buffer.len() >= 1920 {
                        let frame: Vec<f32> = audio_buffer.drain(..1920).collect();
                        match engine.transcribe(&frame, None) {
                            Ok(segments) => {
                                for seg in &segments {
                                    eprintln!("[souffle] Drain segment: {:?} final={}", seg.text, seg.is_final);
                                    on_segment(seg.clone());
                                }
                            }
                            Err(e) => {
                                eprintln!("[souffle] Transcribe error during drain: {e}");
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
                            eprintln!("[souffle] Flush: {} segments", segments.len());
                            for seg in segments {
                                on_segment(seg);
                            }
                        }
                        Err(e) => eprintln!("[souffle] Flush error: {e}"),
                    }
                    return;
                }
                Ok(InferenceCommand::Shutdown) => return,
                _ => {}
            }

            // Read audio with short timeout
            match audio_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(chunk) => {
                    audio_buffer.extend_from_slice(&chunk);

                    // Process complete 1920-sample frames
                    while audio_buffer.len() >= 1920 {
                        let frame: Vec<f32> = audio_buffer.drain(..1920).collect();
                        match engine.transcribe(&frame, None) {
                            Ok(segments) => {
                                for seg in &segments {
                                    eprintln!("[souffle] Segment: {:?} final={}", seg.text, seg.is_final);
                                    on_segment(seg.clone());
                                }
                            }
                            Err(e) => {
                                eprintln!("[souffle] Transcribe error: {e}");
                                return;
                            }
                        }
                        frames_processed += 1;
                        if frames_processed % 50 == 0 {
                            eprintln!(
                                "[souffle] Processed {frames_processed} frames ({:.1}s)",
                                frames_processed as f64 * 0.08,
                            );
                        }
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    eprintln!("[souffle] Audio channel disconnected");
                    return;
                }
            }
        }
    }

    fn process_frames(
        engine: &KyutaiEngine,
        audio: &[f32],
        on_segment: &SegmentCallback,
    ) {
        match engine.transcribe(audio, None) {
            Ok(segments) => {
                for seg in segments {
                    on_segment(seg);
                }
            }
            Err(e) => eprintln!("[souffle] Transcribe error: {e}"),
        }
    }
}
