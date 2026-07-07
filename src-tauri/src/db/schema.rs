use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::engine::TranscriptionProfile;
use crate::transcript::{legacy_recording_session, resolve_legacy_transcription_profile};

/// Schema version 8: calendar_event_id + participants columns on meetings
pub const SCHEMA_VERSION: i64 = 8;

pub const CREATE_SCHEMA_VERSION: &str = "
    CREATE TABLE IF NOT EXISTS schema_version (
        version INTEGER NOT NULL
    );
";

pub const CREATE_MEETINGS_V1: &str = "
    CREATE TABLE IF NOT EXISTS meetings (
        id TEXT PRIMARY KEY,
        title TEXT NOT NULL,
        started_at TEXT NOT NULL,
        ended_at TEXT,
        duration_seconds REAL NOT NULL,
        engine TEXT NOT NULL,
        summary TEXT,
        summary_model TEXT,
        summary_generated_at TEXT,
        edited_transcript TEXT
    );
";

pub const CREATE_MEETINGS_V3: &str = "
    CREATE TABLE IF NOT EXISTS meetings (
        id TEXT PRIMARY KEY,
        title TEXT NOT NULL,
        started_at TEXT NOT NULL,
        ended_at TEXT,
        duration_seconds REAL NOT NULL,
        transcription_profile TEXT NOT NULL,
        recording_sessions TEXT NOT NULL,
        summary TEXT,
        summary_is_stale INTEGER NOT NULL DEFAULT 0,
        summary_model TEXT,
        summary_generated_at TEXT,
        edited_transcript TEXT
    );
";

pub const CREATE_SEGMENTS: &str = "
    CREATE TABLE IF NOT EXISTS segments (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        text TEXT NOT NULL,
        start_time REAL NOT NULL,
        end_time REAL NOT NULL,
        is_final INTEGER NOT NULL DEFAULT 1,
        language TEXT,
        confidence REAL,
        sort_order INTEGER NOT NULL
    );
";

pub const CREATE_SEGMENTS_INDEX: &str = "
    CREATE INDEX IF NOT EXISTS idx_segments_meeting ON segments(meeting_id);
";

pub const CREATE_DICTATION_ENTRIES: &str = "
    CREATE TABLE IF NOT EXISTS dictation_entries (
        id TEXT PRIMARY KEY,
        text TEXT NOT NULL,
        timestamp TEXT NOT NULL
    );
";

pub const CREATE_SETTINGS: &str = "
    CREATE TABLE IF NOT EXISTS settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
";

/// FTS5 table — content-storing (v4+). Enables snippet()/highlight().
pub const CREATE_TEXT_SEARCH: &str = "
    CREATE VIRTUAL TABLE IF NOT EXISTS text_search USING fts5(
        content, source_type, source_id
    );
";

/// Legacy contentless FTS5 table (v1-v3). Kept for v1 schema creation path
/// so that fresh databases go straight to v4 migration which recreates it.
pub const CREATE_TEXT_SEARCH_V1: &str = "
    CREATE VIRTUAL TABLE IF NOT EXISTS text_search USING fts5(
        content, source_type, source_id, content=''
    );
";

pub const CREATE_EMBEDDINGS: &str = "
    CREATE TABLE IF NOT EXISTS embeddings (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        chunk_text TEXT NOT NULL,
        embedding BLOB NOT NULL,
        model_name TEXT NOT NULL,
        dimensions INTEGER NOT NULL,
        created_at TEXT NOT NULL
    );
";

pub const CREATE_EMBEDDINGS_INDEX: &str = "
    CREATE INDEX IF NOT EXISTS idx_embeddings_meeting ON embeddings(meeting_id);
";

pub const CREATE_DICTIONARY: &str = "
    CREATE TABLE IF NOT EXISTS dictionary (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        term TEXT NOT NULL COLLATE NOCASE,
        phonetic_code TEXT,
        category TEXT,
        created_at TEXT NOT NULL,
        UNIQUE(term)
    );
";

/// All schema creation statements in order
pub const SCHEMA_V1: &[&str] = &[
    CREATE_SCHEMA_VERSION,
    CREATE_MEETINGS_V1,
    CREATE_SEGMENTS,
    CREATE_SEGMENTS_INDEX,
    CREATE_DICTATION_ENTRIES,
    CREATE_SETTINGS,
    CREATE_TEXT_SEARCH_V1,
    CREATE_EMBEDDINGS,
    CREATE_EMBEDDINGS_INDEX,
];

pub const SCHEMA_V2: &[&str] = &["ALTER TABLE meetings ADD COLUMN transcription_profile TEXT;"];

struct LegacyMeetingRow {
    id: String,
    title: String,
    started_at: String,
    ended_at: Option<String>,
    duration_seconds: f64,
    engine: String,
    transcription_profile: Option<String>,
    summary: Option<String>,
    summary_model: Option<String>,
    summary_generated_at: Option<String>,
    edited_transcript: Option<String>,
}

pub fn migrate_meetings_to_v3(conn: &mut Connection) -> Result<(), String> {
    conn.execute_batch("PRAGMA foreign_keys=OFF;")
        .map_err(|e| format!("Disable foreign keys for v3 migration: {e}"))?;

    let migration_result = (|| -> Result<(), String> {
        let tx = conn
            .transaction()
            .map_err(|e| format!("Begin v3 migration transaction: {e}"))?;

        tx.execute_batch(
            "
            DROP TABLE IF EXISTS meetings_legacy;
            DROP TABLE IF EXISTS segments_legacy;
            DROP TABLE IF EXISTS embeddings_legacy;
            ALTER TABLE meetings RENAME TO meetings_legacy;
            ALTER TABLE segments RENAME TO segments_legacy;
            ALTER TABLE embeddings RENAME TO embeddings_legacy;
            DROP INDEX IF EXISTS idx_segments_meeting;
            DROP INDEX IF EXISTS idx_embeddings_meeting;
            ",
        )
        .map_err(|e| format!("Rename meetings table for v3 migration: {e}"))?;

        tx.execute_batch(CREATE_MEETINGS_V3)
            .map_err(|e| format!("Create v3 meetings table: {e}"))?;
        tx.execute_batch(CREATE_SEGMENTS)
            .map_err(|e| format!("Create v3 segments table: {e}"))?;
        tx.execute_batch(CREATE_SEGMENTS_INDEX)
            .map_err(|e| format!("Create v3 segments index: {e}"))?;
        tx.execute_batch(CREATE_EMBEDDINGS)
            .map_err(|e| format!("Create v3 embeddings table: {e}"))?;
        tx.execute_batch(CREATE_EMBEDDINGS_INDEX)
            .map_err(|e| format!("Create v3 embeddings index: {e}"))?;

        let mut stmt = tx
            .prepare(
                "SELECT id, title, started_at, ended_at, duration_seconds, engine, transcription_profile, summary, summary_model, summary_generated_at, edited_transcript
                 FROM meetings_legacy",
            )
            .map_err(|e| format!("Prepare legacy meetings query: {e}"))?;

        let legacy_rows = stmt
            .query_map([], |row| {
                Ok(LegacyMeetingRow {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    started_at: row.get(2)?,
                    ended_at: row.get(3)?,
                    duration_seconds: row.get(4)?,
                    engine: row.get(5)?,
                    transcription_profile: row.get(6)?,
                    summary: row.get(7)?,
                    summary_model: row.get(8)?,
                    summary_generated_at: row.get(9)?,
                    edited_transcript: row.get(10)?,
                })
            })
            .map_err(|e| format!("Query legacy meetings: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Collect legacy meetings: {e}"))?;
        drop(stmt);

        for row in legacy_rows {
            let transcription_profile = row
                .transcription_profile
                .as_deref()
                .map(serde_json::from_str::<TranscriptionProfile>)
                .transpose()
                .map_err(|e| {
                    format!(
                        "Deserialize legacy transcription profile for '{}': {e}",
                        row.id
                    )
                })?;
            let transcription_profile =
                resolve_legacy_transcription_profile(transcription_profile, Some(&row.engine))?;

            let started_at = parse_datetime(&row.started_at)?;
            let ended_at = row.ended_at.as_deref().map(parse_datetime).transpose()?;
            let segment_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM segments WHERE meeting_id = ?1",
                    params![row.id],
                    |segment_row| segment_row.get(0),
                )
                .map_err(|e| format!("Count segments for '{}': {e}", row.id))?;
            let recording_sessions = vec![legacy_recording_session(
                &row.id,
                started_at,
                ended_at,
                row.duration_seconds,
                segment_count.max(0) as usize,
            )];

            tx.execute(
                "INSERT INTO meetings (id, title, started_at, ended_at, duration_seconds, transcription_profile, recording_sessions, summary, summary_is_stale, summary_model, summary_generated_at, edited_transcript)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    row.id,
                    row.title,
                    row.started_at,
                    row.ended_at,
                    row.duration_seconds,
                    serde_json::to_string(&transcription_profile)
                        .map_err(|e| format!("Serialize migrated transcription profile: {e}"))?,
                    serde_json::to_string(&recording_sessions)
                        .map_err(|e| format!("Serialize migrated recording sessions: {e}"))?,
                    row.summary,
                    0,
                    row.summary_model,
                    row.summary_generated_at,
                    row.edited_transcript,
                ],
            )
            .map_err(|e| format!("Insert migrated meeting: {e}"))?;
        }

        tx.execute(
            "INSERT INTO segments (id, meeting_id, text, start_time, end_time, is_final, language, confidence, sort_order)
             SELECT id, meeting_id, text, start_time, end_time, is_final, language, confidence, sort_order
             FROM segments_legacy",
            [],
        )
        .map_err(|e| format!("Copy legacy segments: {e}"))?;

        tx.execute(
            "INSERT INTO embeddings (id, meeting_id, chunk_text, embedding, model_name, dimensions, created_at)
             SELECT id, meeting_id, chunk_text, embedding, model_name, dimensions, created_at
             FROM embeddings_legacy",
            [],
        )
        .map_err(|e| format!("Copy legacy embeddings: {e}"))?;

        tx.execute_batch(
            "
            DROP TABLE meetings_legacy;
            DROP TABLE segments_legacy;
            DROP TABLE embeddings_legacy;
            ",
        )
        .map_err(|e| format!("Drop legacy tables after v3 migration: {e}"))?;

        tx.commit()
            .map_err(|e| format!("Commit v3 migration: {e}"))?;

        Ok(())
    })();

    let foreign_keys_result = conn
        .execute_batch("PRAGMA foreign_keys=ON;")
        .map_err(|e| format!("Re-enable foreign keys after v3 migration: {e}"));

    migration_result?;
    foreign_keys_result?;
    Ok(())
}

/// Migrate FTS5 text_search from contentless (content='') to content-storing.
/// Drops the old table, recreates with stored content, and re-indexes all
/// existing meetings and dictation entries.
pub fn migrate_text_search_to_v4(conn: &mut Connection) -> Result<(), String> {
    let tx = conn
        .transaction()
        .map_err(|e| format!("Begin v4 migration transaction: {e}"))?;

    // Drop the contentless FTS5 table and recreate as content-storing
    tx.execute_batch("DROP TABLE IF EXISTS text_search;")
        .map_err(|e| format!("Drop contentless text_search: {e}"))?;

    tx.execute_batch(CREATE_TEXT_SEARCH)
        .map_err(|e| format!("Create content-storing text_search: {e}"))?;

    // Re-index all meetings: concatenate segment texts per meeting
    tx.execute_batch(
        "INSERT INTO text_search (content, source_type, source_id)
         SELECT GROUP_CONCAT(s.text, ' '), 'meeting', m.id
         FROM meetings m
         JOIN segments s ON s.meeting_id = m.id
         GROUP BY m.id
         HAVING GROUP_CONCAT(s.text, ' ') != ''",
    )
    .map_err(|e| format!("Re-index meetings in FTS: {e}"))?;

    // Re-index all dictation entries
    tx.execute_batch(
        "INSERT INTO text_search (content, source_type, source_id)
         SELECT text, 'dictation', id
         FROM dictation_entries
         WHERE text != ''",
    )
    .map_err(|e| format!("Re-index dictation in FTS: {e}"))?;

    tx.commit()
        .map_err(|e| format!("Commit v4 migration: {e}"))?;

    Ok(())
}

fn parse_datetime(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| format!("Parse datetime '{value}': {e}"))
}

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, params};

    use super::{
        CREATE_DICTATION_ENTRIES, CREATE_EMBEDDINGS, CREATE_EMBEDDINGS_INDEX, CREATE_MEETINGS_V1,
        CREATE_SCHEMA_VERSION, CREATE_SEGMENTS, CREATE_SEGMENTS_INDEX, CREATE_SETTINGS,
        CREATE_TEXT_SEARCH_V1, SCHEMA_V2,
    };
    use crate::db::Database;
    use tempfile::TempDir;

    #[test]
    fn fresh_db_creation() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        // Verify tables exist by doing basic operations
        db.get_all_settings().unwrap();
        db.list_meetings().unwrap();
        db.list_dictation_entries(10).unwrap();
    }

    #[test]
    fn idempotent_schema_rerun() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        // Open twice — second open should not fail
        let _db1 = Database::open(&db_path).unwrap();
        let db2 = Database::open(&db_path).unwrap();
        db2.get_all_settings().unwrap();
    }

    #[test]
    fn v2_meetings_migrate_to_v3_schema() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(CREATE_SCHEMA_VERSION).unwrap();
        conn.execute_batch(CREATE_MEETINGS_V1).unwrap();
        conn.execute_batch(CREATE_SEGMENTS).unwrap();
        conn.execute_batch(CREATE_SEGMENTS_INDEX).unwrap();
        conn.execute_batch(CREATE_DICTATION_ENTRIES).unwrap();
        conn.execute_batch(CREATE_SETTINGS).unwrap();
        conn.execute_batch(CREATE_TEXT_SEARCH_V1).unwrap();
        conn.execute_batch(CREATE_EMBEDDINGS).unwrap();
        conn.execute_batch(CREATE_EMBEDDINGS_INDEX).unwrap();
        conn.execute_batch(SCHEMA_V2[0]).unwrap();
        conn.execute("INSERT INTO schema_version (version) VALUES (2)", [])
            .unwrap();
        conn.execute(
            "INSERT INTO meetings (id, title, started_at, ended_at, duration_seconds, engine, transcription_profile, summary, summary_model, summary_generated_at, edited_transcript)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                "legacy-meeting",
                "Legacy Meeting",
                "2026-03-01T10:00:00Z",
                "2026-03-01T10:10:00Z",
                600.0,
                "Custom Engine",
                None::<String>,
                Some("Old summary"),
                Some("qwen"),
                Some("2026-03-01T10:11:00Z"),
                None::<String>,
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO segments (meeting_id, text, start_time, end_time, is_final, language, confidence, sort_order)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params!["legacy-meeting", "Hello", 0.0, 1.0, 1, Some("en"), None::<f32>, 0],
        )
        .unwrap();
        drop(conn);

        let db = Database::open(&db_path).unwrap();
        let meeting = db.load_meeting("legacy-meeting").unwrap();
        assert_eq!(meeting.transcription_profile.engine_label, "Custom Engine");
        assert_eq!(meeting.recording_sessions.len(), 1);
        assert!(!meeting.summary_is_stale);
        // v8 chain ran on the same open: pre-v8 rows read back cleanly.
        assert_eq!(meeting.calendar_event_id, None);
        assert!(meeting.participants.is_empty());

        let conn = db.conn.lock().unwrap();
        let columns: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(meetings)").unwrap();
            stmt.query_map([], |row| row.get(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert!(!columns.iter().any(|column| column == "engine"));
        assert!(columns.iter().any(|column| column == "recording_sessions"));
        assert!(columns.iter().any(|column| column == "summary_is_stale"));
        assert!(columns.iter().any(|column| column == "calendar_event_id"));
        assert!(columns.iter().any(|column| column == "participants"));
    }
}
