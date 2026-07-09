//! Schema-drift contract test between the app's SQLite writer (`souffle_lib::db`)
//! and the MCP sidecar's independent read-only layer (`souffle_mcp::db`).
//!
//! `souffle-mcp` deliberately does not depend on this crate (it must build
//! without pulling in Tauri/candle/ort), so it re-implements its own SQL
//! against the same schema. That duplication is exactly what can drift
//! silently when the app's schema changes. This test writes a meeting and a
//! dictation entry through the real app `Database`, then reads them back
//! through the sidecar's `McpDb`, and asserts every field round-trips.

use chrono::Utc;
use souffle_lib::db::Database;
use souffle_lib::engine::{TranscriptionProfile, TranscriptionSegment};
use souffle_lib::transcript::{MeetingParticipant, MeetingRecordingSession, MeetingTranscript};
use souffle_mcp::db::{IncludeSet, McpDb};
use tempfile::TempDir;

fn build_meeting() -> MeetingTranscript {
    let started_at = Utc::now();
    let ended_at = started_at + chrono::Duration::seconds(120);

    MeetingTranscript {
        id: "contract-1".to_string(),
        title: "Contract Test Meeting".to_string(),
        started_at,
        ended_at: Some(ended_at),
        duration_seconds: 120.0,
        transcription_profile: TranscriptionProfile::default(),
        recording_sessions: vec![MeetingRecordingSession::completed(
            "contract-1-session".to_string(),
            started_at,
            ended_at,
            0,
            2,
        )],
        segments: vec![
            TranscriptionSegment {
                text: "Hello from the contract test meeting.".to_string(),
                start_time: 0.0,
                end_time: 2.0,
                is_final: true,
                language: Some("en".to_string()),
                confidence: Some(0.95),
                speaker: None,
            },
            TranscriptionSegment {
                text: "This checks schema drift end to end.".to_string(),
                start_time: 2.5,
                end_time: 4.0,
                is_final: true,
                language: Some("en".to_string()),
                confidence: Some(0.9),
                speaker: None,
            },
        ],
        summary: Some("A short summary of the contract test meeting.".to_string()),
        summary_is_stale: false,
        summary_model: Some("qwen2.5".to_string()),
        summary_generated_at: Some(ended_at),
        edited_transcript: None,
        notes: Some("Remember to check the schema.".to_string()),
        calendar_event_id: Some("evt-contract".to_string()),
        participants: vec![MeetingParticipant {
            name: "Alice Martin".to_string(),
            email: Some("alice@example.com".to_string()),
            is_organizer: true,
            is_current_user: false,
        }],
    }
}

#[test]
fn sidecar_round_trips_data_written_by_the_real_app() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("souffle.db");

    // Write through the real app database, using the real writers — this is
    // the source of truth for what the schema actually looks like.
    let app_db = Database::open(&db_path).unwrap();
    let meeting = build_meeting();
    app_db.save_meeting(&meeting).unwrap();
    app_db
        .add_dictation_entry("dict-1", "Buy milk on the way home", "2026-01-01T09:00:00+00:00")
        .unwrap();
    drop(app_db);

    // Read back through the sidecar's independent read layer.
    let sidecar = McpDb::open(&db_path).unwrap();

    let list = sidecar.list_meetings(None, None, None, 10).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "contract-1");
    assert_eq!(list[0].title, "Contract Test Meeting");
    assert_eq!(list[0].participants, vec!["Alice Martin".to_string()]);
    assert!(list[0].has_summary);
    assert!(list[0].has_notes);

    let detail = sidecar.get_meeting("contract-1", IncludeSet::all()).unwrap();
    assert_eq!(detail.title, "Contract Test Meeting");
    let transcript = detail.transcript.unwrap();
    assert!(transcript.contains("Hello from the contract test meeting."));
    assert!(transcript.contains("This checks schema drift end to end."));
    assert_eq!(
        detail.summary.as_deref(),
        Some("A short summary of the contract test meeting.")
    );
    assert_eq!(detail.notes.as_deref(), Some("Remember to check the schema."));
    let metadata = detail.metadata.unwrap();
    assert_eq!(metadata.calendar_event_id.as_deref(), Some("evt-contract"));
    assert_eq!(metadata.participants.len(), 1);
    assert_eq!(metadata.participants[0].name, "Alice Martin");
    assert_eq!(metadata.participants[0].email.as_deref(), Some("alice@example.com"));
    assert!(metadata.participants[0].is_organizer);
    assert_eq!(metadata.summary_model.as_deref(), Some("qwen2.5"));
    assert_eq!(metadata.segment_count, 2);

    let latest = sidecar.latest_meeting(IncludeSet::all()).unwrap();
    assert_eq!(latest.id, "contract-1");

    let hits = sidecar.search_meetings("schema drift", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "contract-1");

    let dictations = sidecar.list_dictations(10).unwrap();
    assert_eq!(dictations.len(), 1);
    assert_eq!(dictations[0].id, "dict-1");
    assert_eq!(dictations[0].text, "Buy milk on the way home");
}

#[test]
fn sidecar_get_meeting_include_filter_matches_across_the_boundary() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("souffle.db");

    let app_db = Database::open(&db_path).unwrap();
    app_db.save_meeting(&build_meeting()).unwrap();
    drop(app_db);

    let sidecar = McpDb::open(&db_path).unwrap();
    let names = vec!["summary".to_string(), "notes".to_string()];
    let detail = sidecar
        .get_meeting("contract-1", IncludeSet::from_names(Some(&names)))
        .unwrap();

    assert!(detail.transcript.is_none());
    assert!(detail.metadata.is_none());
    assert!(detail.summary.is_some());
    assert!(detail.notes.is_some());
}
