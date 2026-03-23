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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::TranscriptionSegment;

    #[test]
    fn meeting_transcript_serialization_round_trip() {
        let meeting = MeetingTranscript {
            id: "test-id".to_string(),
            title: "Test".to_string(),
            started_at: Utc::now(),
            ended_at: None,
            duration_seconds: 30.0,
            engine: "test".to_string(),
            segments: vec![TranscriptionSegment {
                text: "hello".to_string(),
                start_time: 0.0,
                end_time: 1.0,
                is_final: true,
                language: None,
                confidence: None,
            }],
            summary: Some("A summary".to_string()),
            summary_model: Some("test-model".to_string()),
            summary_generated_at: None,
        };

        let json = serde_json::to_string(&meeting).unwrap();
        let deserialized: MeetingTranscript = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "test-id");
        assert_eq!(deserialized.segments.len(), 1);
        assert_eq!(deserialized.summary.as_deref(), Some("A summary"));
    }

    #[test]
    fn meeting_list_item_from_transcript() {
        let transcript = MeetingTranscript {
            id: "m1".to_string(),
            title: "My Meeting".to_string(),
            started_at: Utc::now(),
            ended_at: None,
            duration_seconds: 120.0,
            engine: "test".to_string(),
            segments: vec![],
            summary: Some("summary".to_string()),
            summary_model: None,
            summary_generated_at: None,
        };

        let item = MeetingListItem::from(&transcript);
        assert_eq!(item.id, "m1");
        assert_eq!(item.title, "My Meeting");
        assert!(item.has_summary);
    }
}
