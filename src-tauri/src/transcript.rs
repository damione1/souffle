use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::engine::TranscriptionSegment;

/// Full meeting transcript stored as JSON
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingTranscript {
    pub id: String,
    pub title: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_seconds: f64,
    pub engine: String,
    pub segments: Vec<TranscriptionSegment>,
    pub summary: Option<String>,
    pub summary_model: Option<String>,
    pub summary_generated_at: Option<DateTime<Utc>>,
}

/// Lightweight item for listing meetings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingListItem {
    pub id: String,
    pub title: String,
    pub started_at: DateTime<Utc>,
    pub duration_seconds: f64,
    pub has_summary: bool,
}

impl From<&MeetingTranscript> for MeetingListItem {
    fn from(t: &MeetingTranscript) -> Self {
        Self {
            id: t.id.clone(),
            title: t.title.clone(),
            started_at: t.started_at,
            duration_seconds: t.duration_seconds,
            has_summary: t.summary.is_some(),
        }
    }
}
