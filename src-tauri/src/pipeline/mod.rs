//! Transcription pipeline: the engine actor and its session types.

mod actor;
mod health;

pub use actor::{
    EngineActorHandle, EngineCommand, EngineFactory, EngineInfo, SessionConfig, SessionSummary,
};

use crate::engine::TranscriptionSegment;

/// Callback type for streaming segments to the frontend
pub type SegmentCallback = Box<dyn Fn(TranscriptionSegment) + Send + 'static>;
