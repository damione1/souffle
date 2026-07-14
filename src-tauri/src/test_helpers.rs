// Shared test helpers — #[cfg(test)] only
// Consolidates duplicated test_db() pattern from 4+ modules

#[cfg(test)]
pub mod fixtures {
    use crate::db::Database;
    use crate::engine::{TranscriptionProfile, TranscriptionSegment};
    use crate::transcript::{MeetingRecordingSession, MeetingTranscript};
    use chrono::Utc;
    use tempfile::TempDir;

    /// Creates a temporary database for testing. Returns (Database, TempDir).
    /// Keep TempDir in scope to prevent cleanup.
    pub fn test_db() -> (Database, TempDir) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        (db, dir)
    }

    /// Creates a sample MeetingTranscript with the given id.
    pub fn sample_meeting(id: &str) -> MeetingTranscript {
        let started_at = Utc::now();
        let ended_at = started_at + chrono::Duration::seconds(60);

        MeetingTranscript {
            id: id.to_string(),
            title: "Test Meeting".to_string(),
            started_at,
            ended_at: Some(ended_at),
            duration_seconds: 60.0,
            transcription_profile: TranscriptionProfile::default(),
            recording_sessions: vec![MeetingRecordingSession::completed(
                format!("{id}-session-1"),
                started_at,
                ended_at,
                0,
                2,
            )],
            segments: vec![
                TranscriptionSegment {
                    text: "Hello world".to_string(),
                    start_time: 0.0,
                    end_time: 1.0,
                    is_final: true,
                    language: Some("en".to_string()),
                    confidence: Some(0.95),
                    speaker: None,
                },
                TranscriptionSegment {
                    text: "second segment".to_string(),
                    start_time: 1.5,
                    end_time: 2.5,
                    is_final: true,
                    language: None,
                    confidence: None,
                    speaker: None,
                },
            ],
            summary: None,
            summary_is_stale: false,
            summary_model: None,
            summary_generated_at: None,
            structured_summary: None,
            edited_transcript: None,
            notes: None,
            calendar_event_id: None,
            participants: Vec::new(),
            speakers: Vec::new(),
        }
    }

    /// Creates a sample TranscriptionSegment.
    pub fn sample_segment(text: &str, start: f64, end: f64) -> TranscriptionSegment {
        TranscriptionSegment {
            text: text.to_string(),
            start_time: start,
            end_time: end,
            is_final: true,
            language: Some("en".to_string()),
            confidence: Some(0.95),
            speaker: None,
        }
    }
}
