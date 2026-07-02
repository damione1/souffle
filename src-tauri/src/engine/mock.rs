//! Mock transcription engine for testing pipeline behavior without GPU/hardware.
#![cfg(test)]

use super::{
    AudioInputRequirements, EngineError, Speaker, TranscriptionEngine, TranscriptionSegment,
};
use std::collections::VecDeque;
use std::path::Path;

/// A configurable mock engine for testing.
/// Push responses into `transcribe_responses` and `flush_responses` queues;
/// calls to `transcribe()` / `flush()` will pop from the front.
pub struct MockEngine {
    loaded: bool,
    pub transcribe_responses: VecDeque<Result<Vec<TranscriptionSegment>, EngineError>>,
    pub flush_responses: VecDeque<Result<Vec<TranscriptionSegment>, EngineError>>,
}

impl Default for MockEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MockEngine {
    pub fn new() -> Self {
        Self {
            loaded: false,
            transcribe_responses: VecDeque::new(),
            flush_responses: VecDeque::new(),
        }
    }

    /// Convenience: pre-load N identical transcribe responses.
    pub fn with_transcribe_response(
        mut self,
        resp: Result<Vec<TranscriptionSegment>, EngineError>,
        count: usize,
    ) -> Self {
        for _ in 0..count {
            self.transcribe_responses.push_back(match &resp {
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
        self
    }

    /// Convenience: pre-load a single flush response.
    pub fn with_flush_response(
        mut self,
        resp: Result<Vec<TranscriptionSegment>, EngineError>,
    ) -> Self {
        self.flush_responses.push_back(resp);
        self
    }
}

impl TranscriptionEngine for MockEngine {
    fn load_model(&mut self, _path: &Path) -> Result<(), EngineError> {
        self.loaded = true;
        Ok(())
    }

    fn unload_model(&mut self) -> Result<(), EngineError> {
        self.loaded = false;
        Ok(())
    }

    fn transcribe(
        &mut self,
        _audio: &[f32],
        _language: Option<&str>,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        self.transcribe_responses.pop_front().unwrap_or(Ok(vec![]))
    }

    fn flush(&mut self) -> Result<Vec<TranscriptionSegment>, EngineError> {
        self.flush_responses.pop_front().unwrap_or(Ok(vec![]))
    }

    fn reset_state(&mut self) -> Result<(), EngineError> {
        Ok(())
    }

    fn audio_requirements(&self) -> AudioInputRequirements {
        AudioInputRequirements {
            sample_rate_hz: crate::constants::SAMPLE_RATE,
            channels: 1,
            chunk_size_samples: crate::constants::MIMI_FRAME_SIZE as u32,
        }
    }

    fn supports_diarization(&self) -> bool {
        true
    }

    /// Emit a speaker-tagged segment for each lane that carries non-silent
    /// audio, so tests can assert the diarized loop routes by speaker.
    fn transcribe_dual(
        &mut self,
        me: &[f32],
        them: &[f32],
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let tagged = |speaker: Speaker, text: &str| TranscriptionSegment {
            text: text.to_string(),
            start_time: 0.0,
            end_time: 0.0,
            is_final: true,
            language: None,
            confidence: None,
            speaker: Some(speaker),
        };
        let mut segments = Vec::new();
        if me.iter().any(|s| *s != 0.0) {
            segments.push(tagged(Speaker::Me, "me-speaks"));
        }
        if them.iter().any(|s| *s != 0.0) {
            segments.push(tagged(Speaker::Them, "them-speaks"));
        }
        Ok(segments)
    }
}
