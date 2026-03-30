//! Mock transcription engine for testing pipeline behavior without GPU/hardware.
#![cfg(test)]

use super::{AudioInputRequirements, EngineError, TranscriptionEngine, TranscriptionSegment};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// A configurable mock engine for testing.
/// Push responses into `transcribe_responses` and `flush_responses` queues;
/// calls to `transcribe()` / `flush()` will pop from the front.
pub struct MockEngine {
    loaded: AtomicBool,
    pub transcribe_responses: Mutex<VecDeque<Result<Vec<TranscriptionSegment>, EngineError>>>,
    pub flush_responses: Mutex<VecDeque<Result<Vec<TranscriptionSegment>, EngineError>>>,
}

impl Default for MockEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MockEngine {
    pub fn new() -> Self {
        Self {
            loaded: AtomicBool::new(false),
            transcribe_responses: Mutex::new(VecDeque::new()),
            flush_responses: Mutex::new(VecDeque::new()),
        }
    }

    /// Convenience: pre-load N identical transcribe responses.
    pub fn with_transcribe_response(
        self,
        resp: Result<Vec<TranscriptionSegment>, EngineError>,
        count: usize,
    ) -> Self {
        let mut queue = self.transcribe_responses.lock().unwrap();
        for _ in 0..count {
            queue.push_back(match &resp {
                Ok(segments) => Ok(segments.clone()),
                Err(e) => Err(match e {
                    EngineError::ModelNotFound(p) => EngineError::ModelNotFound(p.clone()),
                    EngineError::LoadError(s) => EngineError::LoadError(s.clone()),
                    EngineError::InferenceError(s) => EngineError::InferenceError(s.clone()),
                    EngineError::UnsupportedLanguage(s) => {
                        EngineError::UnsupportedLanguage(s.clone())
                    }
                    EngineError::NotInitialized => EngineError::NotInitialized,
                    EngineError::OutOfMemory => EngineError::OutOfMemory,
                }),
            });
        }
        drop(queue);
        self
    }

    /// Convenience: pre-load a single flush response.
    pub fn with_flush_response(
        self,
        resp: Result<Vec<TranscriptionSegment>, EngineError>,
    ) -> Self {
        self.flush_responses.lock().unwrap().push_back(resp);
        self
    }
}

impl TranscriptionEngine for MockEngine {
    fn load_model(&mut self, _path: &Path) -> Result<(), EngineError> {
        self.loaded.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn unload_model(&mut self) -> Result<(), EngineError> {
        self.loaded.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn transcribe(
        &self,
        _audio: &[f32],
        _language: Option<&str>,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        self.transcribe_responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(vec![]))
    }

    fn flush(&self) -> Result<Vec<TranscriptionSegment>, EngineError> {
        self.flush_responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(vec![]))
    }

    fn reset_state(&self) -> Result<(), EngineError> {
        Ok(())
    }

    fn audio_requirements(&self) -> AudioInputRequirements {
        AudioInputRequirements {
            sample_rate_hz: crate::constants::SAMPLE_RATE,
            channels: 1,
            chunk_size_samples: crate::constants::MIMI_FRAME_SIZE as u32,
        }
    }
}
