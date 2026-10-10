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
use souffle_lib::transcript::{
    MeetingParticipant, MeetingRecordingSession, MeetingTranscript, StructuredActionItem,
    StructuredSummary,
};
use souffle_mcp::db::{IncludeSet, McpDb};
use souffle_schema::sql::{column as col, table};
use tempfile::TempDir;

fn build_meeting(id: &str, title: &str, is_ongoing: bool) -> MeetingTranscript {
    let started_at = Utc::now();
    let ended_at = if is_ongoing {
        None
    } else {
        Some(started_at + chrono::Duration::seconds(120))
    };

    MeetingTranscript {
        id: id.to_string(),
        title: title.to_string(),
        started_at,
        ended_at,
        duration_seconds: 120.0,
        transcription_profile: TranscriptionProfile::default(),
        recording_sessions: vec![MeetingRecordingSession::completed(
            "contract-1-session".to_string(),
            started_at,
            if is_ongoing {
                started_at
            } else {
                ended_at.unwrap()
            },
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
        summary_generated_at: ended_at,
        structured_summary: Some(StructuredSummary {
            decisions: vec!["Proceed with schema contract test".to_string()],
            action_items: vec![StructuredActionItem {
                text: "Keep MCP in sync".to_string(),
                owner: Some("Alice Martin".to_string()),
            }],
            open_questions: vec!["Any drift?".to_string()],
        }),
        edited_transcript: None,
        notes: Some("Remember to check the schema.".to_string()),
        calendar_event_id: Some("evt-contract".to_string()),
        participants: vec![MeetingParticipant {
            name: "Alice Martin".to_string(),
            email: Some("alice@example.com".to_string()),
            is_organizer: true,
            is_current_user: false,
        }],
        system_audio: None,
    }
}

#[test]
fn sidecar_round_trips_data_written_by_the_real_app() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join(souffle_schema::DB_FILENAME);

    // Write through the real app database, using the real writers — this is
    // the source of truth for what the schema actually looks like.
    let app_db = Database::open(&db_path).unwrap();
    let meeting = build_meeting("contract-1", "Contract Test Meeting", false);
    app_db.save_meeting(&meeting).unwrap();
    let ongoing_meeting = build_meeting("contract-ongoing", "Ongoing Test Meeting", true);
    app_db.save_meeting(&ongoing_meeting).unwrap();
    app_db
        .add_dictation_entry(
            "dict-1",
            "Buy milk on the way home",
            "2026-01-01T09:00:00+00:00",
        )
        .unwrap();
    drop(app_db);

    // Read back through the sidecar's independent read layer.
    let sidecar = McpDb::open(&db_path).unwrap();

    let list = sidecar.list_meetings(None, None, None, 10).unwrap();
    assert_eq!(list.len(), 2);
    // ordered by started_at DESC, and ongoing was created second
    assert_eq!(list[0].id, "contract-ongoing");
    assert_eq!(list[0].title, "Ongoing Test Meeting");
    assert!(list[0].is_ongoing);
    assert_eq!(list[0].ended_at, None);

    assert_eq!(list[1].id, "contract-1");
    assert_eq!(list[1].title, "Contract Test Meeting");
    assert_eq!(list[1].participants, vec!["Alice Martin".to_string()]);
    assert!(list[1].has_summary);
    assert!(list[1].has_notes);
    assert!(!list[1].is_ongoing);
    assert!(list[1].ended_at.is_some());

    let detail = sidecar
        .get_meeting("contract-1", IncludeSet::all())
        .unwrap();
    assert_eq!(detail.title, "Contract Test Meeting");
    let transcript = detail.transcript.unwrap();
    assert!(transcript.contains("Hello from the contract test meeting."));
    assert!(transcript.contains("This checks schema drift end to end."));
    assert_eq!(
        detail.summary.as_deref(),
        Some("A short summary of the contract test meeting.")
    );
    let structured = detail.structured_summary.expect("structured summary");
    assert_eq!(
        structured.decisions,
        vec!["Proceed with schema contract test"]
    );
    assert_eq!(structured.action_items.len(), 1);
    assert_eq!(structured.action_items[0].text, "Keep MCP in sync");
    assert_eq!(
        structured.action_items[0].owner.as_deref(),
        Some("Alice Martin")
    );
    assert_eq!(structured.open_questions, vec!["Any drift?"]);
    assert_eq!(
        detail.notes.as_deref(),
        Some("Remember to check the schema.")
    );
    let metadata = detail.metadata.unwrap();
    assert_eq!(metadata.calendar_event_id.as_deref(), Some("evt-contract"));
    assert_eq!(metadata.participants.len(), 1);
    assert_eq!(metadata.participants[0].name, "Alice Martin");
    assert_eq!(
        metadata.participants[0].email.as_deref(),
        Some("alice@example.com")
    );
    assert!(metadata.participants[0].is_organizer);
    assert_eq!(metadata.summary_model.as_deref(), Some("qwen2.5"));
    assert_eq!(metadata.segment_count, 2);

    let ongoing_detail = sidecar
        .get_meeting("contract-ongoing", IncludeSet::all())
        .unwrap();
    assert!(ongoing_detail.is_ongoing);
    assert_eq!(ongoing_detail.ended_at, None);

    let latest = sidecar.latest_meeting(IncludeSet::all()).unwrap();
    assert_eq!(latest.id, "contract-ongoing");

    let hits = sidecar.search_meetings("schema drift", 10).unwrap();
    assert_eq!(hits.len(), 2);
    // hits[0] and hits[1] order can vary depending on FTS ranking or rowid, but both have it

    let dictations = sidecar.list_dictations(10).unwrap();
    assert_eq!(dictations.len(), 1);
    assert_eq!(dictations[0].id, "dict-1");
    assert_eq!(dictations[0].text, "Buy milk on the way home");

    // V17 adds only dictionary_suggestions, an app-only table. Removing it
    // produces the actual V16 schema rather than merely changing its number.
    let v17_outputs = all_tool_outputs(&sidecar);
    assert_eq!(
        sidecar.schema_version().unwrap(),
        souffle_schema::SCHEMA_VERSION
    );
    drop(sidecar);
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch("DROP TABLE dictionary_suggestions;")
        .unwrap();
    conn.execute(
        &format!("UPDATE {} SET {} = ?1", table::SCHEMA_VERSION, col::VERSION),
        [16],
    )
    .unwrap();
    let has_v17_table: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='dictionary_suggestions')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!has_v17_table);
    drop(conn);
    let v16 = McpDb::open(&db_path).unwrap();
    assert_eq!(v16.schema_version().unwrap(), 16);
    assert_eq!(
        all_tool_outputs(&v16),
        v17_outputs,
        "all five MCP tools preserve their V16 output"
    );
}

fn all_tool_outputs(db: &McpDb) -> serde_json::Value {
    serde_json::json!({
        "list_meetings": db.list_meetings(None, None, None, 10).unwrap(),
        "filtered_meetings": db.list_meetings(Some("drift"), Some("1900-01-01"), Some("2099-12-31"), 10).unwrap(),
        "get_meeting": db.get_meeting("contract-1", IncludeSet::all()).unwrap(),
        "latest_meeting": db.latest_meeting(IncludeSet::all()).unwrap(),
        "search_meetings": db.search_meetings("schema drift", 10).unwrap(),
        "list_dictations": db.list_dictations(10).unwrap(),
    })
}

#[test]
fn every_shared_identifier_resolves_against_the_real_app_schema() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join(souffle_schema::DB_FILENAME);
    drop(Database::open(&path).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    for (table, columns) in [
        (table::SCHEMA_VERSION, &[col::VERSION][..]),
        (
            table::MEETINGS,
            &[
                col::ID,
                col::TITLE,
                col::STARTED_AT,
                col::ENDED_AT,
                col::DURATION_SECONDS,
                col::TRANSCRIPTION_PROFILE,
                col::RECORDING_SESSIONS,
                col::SUMMARY,
                col::SUMMARY_IS_STALE,
                col::SUMMARY_MODEL,
                col::SUMMARY_GENERATED_AT,
                col::EDITED_TRANSCRIPT,
                col::NOTES,
                col::CALENDAR_EVENT_ID,
                col::PARTICIPANTS,
                col::STRUCTURED_SUMMARY,
                col::SYSTEM_AUDIO,
            ][..],
        ),
        (
            table::SEGMENTS,
            &[
                col::ID,
                col::MEETING_ID,
                col::TEXT,
                col::START_TIME,
                col::END_TIME,
                col::IS_FINAL,
                col::LANGUAGE,
                col::CONFIDENCE,
                col::SORT_ORDER,
                col::SPEAKER,
            ][..],
        ),
        (
            table::DICTATION_ENTRIES,
            &[col::ID, col::TEXT, col::TIMESTAMP][..],
        ),
        (
            table::TEXT_SEARCH,
            &[col::CONTENT, col::SOURCE_TYPE, col::SOURCE_ID, col::RANK][..],
        ),
    ] {
        conn.prepare(&format!(
            "SELECT {} FROM {table} LIMIT 0",
            columns.join(", ")
        ))
        .unwrap_or_else(|error| {
            panic!("shared identifiers drifted from app table {table}: {error}")
        });
    }
}

#[test]
fn sidecar_get_meeting_include_filter_matches_across_the_boundary() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join(souffle_schema::DB_FILENAME);

    let app_db = Database::open(&db_path).unwrap();
    app_db
        .save_meeting(&build_meeting("contract-1", "Contract Test Meeting", false))
        .unwrap();
    drop(app_db);

    let sidecar = McpDb::open(&db_path).unwrap();
    let names = vec![
        souffle_mcp::db::MeetingSection::Summary,
        souffle_mcp::db::MeetingSection::Notes,
    ];
    let detail = sidecar
        .get_meeting("contract-1", IncludeSet::from_sections(Some(&names)))
        .unwrap();

    assert!(detail.transcript.is_none());
    assert!(detail.metadata.is_none());
    assert!(detail.summary.is_some());
    assert!(detail.notes.is_some());
}
