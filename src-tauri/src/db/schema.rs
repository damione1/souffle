use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::engine::TranscriptionProfile;
use crate::transcript::{legacy_recording_session, resolve_legacy_transcription_profile};

/// Schema version 13: `speaker_embeddings` table for multi-embedding speaker
/// matching, replacing the single running-mean centroid on `speakers`.
pub const SCHEMA_VERSION: i64 = 13;

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

/// Persistent, cross-meeting speaker identities resolved by offline
/// diarization. `centroid`/`embedding_count` are no longer populated by
/// `create_speaker` or read by the matcher as of v13 (matching uses the
/// multi-embedding `speaker_embeddings` table instead of a single
/// running-mean centroid), and `db::speakers::SpeakerRecord` doesn't expose
/// them either. The columns stay in the schema anyway, purely to avoid an
/// ALTER-driven table rebuild: the MCP sidecar (`souffle-mcp`) only reads
/// `name` from this table, so it is not a reason to keep them. `is_me` flags
/// the speaker who is the app's user; at most one row has it set (enforced
/// by `Database::set_speaker_is_me`, not by the schema).
pub const CREATE_SPEAKERS: &str = "
    CREATE TABLE IF NOT EXISTS speakers (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL,
        centroid BLOB,
        embedding_count INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL,
        last_seen_at TEXT NOT NULL,
        is_me INTEGER NOT NULL DEFAULT 0
    );
";

/// Up to `db::speakers::MAX_EMBEDDINGS_PER_SPEAKER` recent voice embeddings
/// per persistent speaker, used to match future clusters by MAX cosine
/// similarity across the bag rather than a single running-mean centroid.
/// `embedding` is an opaque BLOB from this layer's point of view: the
/// diarization crate encodes it as little-endian f32s. No FK cascade (the
/// `PRAGMA foreign_keys` setting is not guaranteed active on every
/// connection): `delete_speaker` removes matching rows explicitly.
pub const CREATE_SPEAKER_EMBEDDINGS: &str = "
    CREATE TABLE IF NOT EXISTS speaker_embeddings (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        speaker_id INTEGER NOT NULL,
        meeting_id TEXT,
        embedding BLOB NOT NULL,
        speech_seconds REAL NOT NULL,
        created_at TEXT NOT NULL
    );
";

pub const CREATE_SPEAKER_EMBEDDINGS_INDEX: &str = "
    CREATE INDEX IF NOT EXISTS idx_speaker_embeddings_speaker ON speaker_embeddings(speaker_id);
";

/// Enforces at most one speaker flagged `is_me` at a time at the schema
/// level, not just by application code (`Database::set_speaker_is_me`
/// already maintains it transactionally, and `Database::merge_speakers`
/// orders its statements to avoid a transient double-`is_me` mid-merge, but
/// the invariant is cheap enough to also guarantee here). A partial index
/// (`WHERE is_me = 1`) only constrains rows where the flag is set, so any
/// number of `is_me = 0` rows coexist freely.
pub const CREATE_SPEAKERS_IS_ME_INDEX: &str = "
    CREATE UNIQUE INDEX IF NOT EXISTS idx_speakers_is_me ON speakers(is_me) WHERE is_me = 1;
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

/// v9: `dictionary.phonetic_code` becomes a user-facing pronunciation
/// spelling. Until now the app auto-filled it with the term's Soundex code;
/// those derived values are cleared (kept only if a caller stored something
/// other than the auto value). The Soundex is recomputed at filter build.
pub fn migrate_dictionary_phonetics_to_v9(conn: &Connection) -> Result<(), String> {
    let auto_derived: Vec<i64> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, term, phonetic_code FROM dictionary WHERE phonetic_code IS NOT NULL",
            )
            .map_err(|e| format!("Prepare v9 dictionary scan: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| format!("Query v9 dictionary scan: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Collect v9 dictionary scan: {e}"))?;
        rows.into_iter()
            .filter(|(_, term, code)| {
                crate::filter::soundex::soundex(term).as_deref() == Some(code)
            })
            .map(|(id, _, _)| id)
            .collect()
    };
    for id in auto_derived {
        conn.execute(
            "UPDATE dictionary SET phonetic_code = NULL WHERE id = ?1",
            params![id],
        )
        .map_err(|e| format!("Clear auto phonetic code (id {id}): {e}"))?;
    }
    Ok(())
}

pub fn migrate_drop_embeddings_to_v10(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS embeddings;
         DROP INDEX IF EXISTS idx_embeddings_meeting;",
    )
    .map_err(|e| format!("Drop embeddings table and index: {e}"))?;

    Ok(())
}

pub fn migrate_add_structured_summary_to_v11(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "ALTER TABLE meetings ADD COLUMN structured_summary TEXT",
        [],
    )
    .map_err(|e| format!("Add structured_summary column: {e}"))?;
    Ok(())
}

/// v12: `speakers` table for persistent, cross-meeting speaker identities.
/// `segments.speaker` values of the form `spk:<id>` reference this table;
/// existing "me"/"them" segments are untouched.
pub fn migrate_add_speakers_to_v12(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(CREATE_SPEAKERS)
        .map_err(|e| format!("Create speakers table: {e}"))?;
    Ok(())
}

/// v13: multi-embedding speaker matching replaces the single running-mean
/// centroid. Every historical match attempt under the old thresholds
/// (0.65 similarity floor, 0.03 margin) failed in practice, so `speakers`
/// rows that no segment ever ended up referencing are purged first: they
/// are pure duplicate-name clutter, not identities worth carrying forward.
/// Surviving speakers with a centroid get it seeded as their first
/// `speaker_embeddings` row, so an existing enrollment isn't lost outright
/// even though the matcher will accumulate fresher, more representative
/// embeddings over time. Also adds `speakers.is_me`, guarded by a column
/// check: a database that just went through the v12 step above already has
/// it (`CREATE_SPEAKERS` includes it), so the `ALTER TABLE` only fires for
/// databases that were already at v12 before this column existed. Finally
/// creates `CREATE_SPEAKERS_IS_ME_INDEX`: this is also where a fresh
/// database picks it up, since `ensure_schema` runs every versioned step in
/// order starting from 0 rather than jumping straight to the latest schema.
pub fn migrate_speaker_embeddings_to_v13(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "DELETE FROM speakers WHERE id NOT IN (
            SELECT DISTINCT CAST(substr(speaker, 5) AS INTEGER)
            FROM segments WHERE speaker LIKE 'spk:%'
        )",
        [],
    )
    .map_err(|e| format!("Purge orphaned speakers: {e}"))?;

    if !speakers_has_is_me_column(conn)? {
        conn.execute("ALTER TABLE speakers ADD COLUMN is_me INTEGER NOT NULL DEFAULT 0", [])
            .map_err(|e| format!("Add is_me column: {e}"))?;
    }

    conn.execute_batch(CREATE_SPEAKER_EMBEDDINGS)
        .map_err(|e| format!("Create speaker_embeddings table: {e}"))?;
    conn.execute_batch(CREATE_SPEAKER_EMBEDDINGS_INDEX)
        .map_err(|e| format!("Create speaker_embeddings index: {e}"))?;
    conn.execute_batch(CREATE_SPEAKERS_IS_ME_INDEX)
        .map_err(|e| format!("Create speakers is_me unique index: {e}"))?;

    conn.execute(
        "INSERT INTO speaker_embeddings (speaker_id, meeting_id, embedding, speech_seconds, created_at)
         SELECT id, NULL, centroid, 0.0, last_seen_at FROM speakers WHERE centroid IS NOT NULL",
        [],
    )
    .map_err(|e| format!("Seed speaker_embeddings from centroids: {e}"))?;

    Ok(())
}

fn speakers_has_is_me_column(conn: &Connection) -> Result<bool, String> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(speakers)")
        .map_err(|e| format!("Prepare speakers table_info: {e}"))?;
    let has_column = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("Query speakers table_info: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Collect speakers table_info: {e}"))?
        .iter()
        .any(|name| name == "is_me");
    Ok(has_column)
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
    fn fresh_db_no_embeddings_table() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let db = Database::open(&db_path).unwrap();

        let conn = db.conn.lock().unwrap();
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='embeddings'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            !table_exists,
            "Fresh database should not have embeddings table in schema v10"
        );

        let index_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name='idx_embeddings_meeting'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            !index_exists,
            "Fresh database should not have idx_embeddings_meeting index in schema v10"
        );
    }

    #[test]
    fn v9_embeddings_migrate_to_v10_schema() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        // Minimal v9-era fixture: only the tables the v10 step touches, plus a
        // v3-shaped meetings table to mirror a real v9 database.
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(CREATE_SCHEMA_VERSION).unwrap();
        conn.execute_batch(super::CREATE_MEETINGS_V3).unwrap();
        conn.execute_batch(CREATE_SEGMENTS).unwrap();
        conn.execute_batch(CREATE_SEGMENTS_INDEX).unwrap();
        // A real v9 database has already been through the v7 migration.
        conn.execute("ALTER TABLE segments ADD COLUMN speaker TEXT", [])
            .unwrap();
        conn.execute_batch(CREATE_DICTATION_ENTRIES).unwrap();
        conn.execute_batch(CREATE_SETTINGS).unwrap();
        conn.execute_batch(super::CREATE_TEXT_SEARCH).unwrap();
        conn.execute_batch(CREATE_EMBEDDINGS).unwrap();
        conn.execute_batch(CREATE_EMBEDDINGS_INDEX).unwrap();
        conn.execute("INSERT INTO schema_version (version) VALUES (9)", [])
            .unwrap();
        conn.execute(
            "INSERT INTO meetings (id, title, started_at, ended_at, duration_seconds, transcription_profile, recording_sessions, summary, summary_is_stale, summary_model, summary_generated_at, edited_transcript)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                "test-meeting",
                "Test Meeting",
                "2026-03-01T10:00:00Z",
                "2026-03-01T10:10:00Z",
                600.0,
                r#"{"engine_id":"parakeet","engine_label":"Parakeet","model_id":"parakeet-tdt-0.6b-v2","model_label":"Parakeet TDT 0.6B v2","backend_id":"ort","backend_label":"ONNX Runtime"}"#,
                "[]",
                None::<String>,
                0,
                None::<String>,
                None::<String>,
                None::<String>,
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO embeddings (meeting_id, chunk_text, embedding, model_name, dimensions, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                "test-meeting",
                "Sample chunk",
                vec![0u8; 512],
                "test-model",
                512,
                "2026-03-01T10:00:00Z",
            ],
        )
        .unwrap();
        drop(conn);

        let db = Database::open(&db_path).unwrap();

        let conn = db.conn.lock().unwrap();
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='embeddings'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            !table_exists,
            "v9 database with embeddings should have table dropped after v10 migration"
        );

        let index_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name='idx_embeddings_meeting'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            !index_exists,
            "v9 database should have idx_embeddings_meeting dropped after v10 migration"
        );

        let meeting_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='meetings'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            meeting_exists,
            "meetings table should still exist after v10 migration"
        );
    }

    #[test]
    fn v10_migrate_to_v11_adds_structured_summary_column() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(CREATE_SCHEMA_VERSION).unwrap();
        conn.execute_batch(super::CREATE_MEETINGS_V3).unwrap();
        conn.execute_batch(CREATE_SEGMENTS).unwrap();
        conn.execute_batch(CREATE_SEGMENTS_INDEX).unwrap();
        // A real v10 database has already been through the v7 migration.
        conn.execute("ALTER TABLE segments ADD COLUMN speaker TEXT", [])
            .unwrap();
        conn.execute_batch(CREATE_DICTATION_ENTRIES).unwrap();
        conn.execute_batch(CREATE_SETTINGS).unwrap();
        conn.execute_batch(super::CREATE_TEXT_SEARCH).unwrap();
        conn.execute("INSERT INTO schema_version (version) VALUES (10)", [])
            .unwrap();
        drop(conn);

        let db = Database::open(&db_path).unwrap();
        let conn = db.conn.lock().unwrap();
        let columns: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(meetings)").unwrap();
            stmt.query_map([], |row| row.get(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert!(
            columns.iter().any(|column| column == "structured_summary"),
            "v11 migration should add structured_summary column"
        );
    }

    #[test]
    fn v11_migrate_to_v12_adds_speakers_table() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        // Minimal v11-era fixture: meetings v3 shape plus the structured_summary
        // column v11 added, no speakers table yet.
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(CREATE_SCHEMA_VERSION).unwrap();
        conn.execute_batch(super::CREATE_MEETINGS_V3).unwrap();
        conn.execute(
            "ALTER TABLE meetings ADD COLUMN structured_summary TEXT",
            [],
        )
        .unwrap();
        conn.execute_batch(CREATE_SEGMENTS).unwrap();
        conn.execute_batch(CREATE_SEGMENTS_INDEX).unwrap();
        // A real v11 database has already been through the v7 migration.
        conn.execute("ALTER TABLE segments ADD COLUMN speaker TEXT", [])
            .unwrap();
        conn.execute_batch(CREATE_DICTATION_ENTRIES).unwrap();
        conn.execute_batch(CREATE_SETTINGS).unwrap();
        conn.execute_batch(super::CREATE_TEXT_SEARCH).unwrap();
        conn.execute("INSERT INTO schema_version (version) VALUES (11)", [])
            .unwrap();
        drop(conn);

        let db = Database::open(&db_path).unwrap();

        let conn = db.conn.lock().unwrap();
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='speakers'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            table_exists,
            "v12 migration should create the speakers table"
        );

        let columns: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(speakers)").unwrap();
            stmt.query_map([], |row| row.get(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        for expected in [
            "id",
            "name",
            "centroid",
            "embedding_count",
            "created_at",
            "last_seen_at",
        ] {
            assert!(
                columns.iter().any(|column| column == expected),
                "speakers table should have column {expected}"
            );
        }
    }

    #[test]
    fn v12_migrate_to_v13_purges_orphans_and_seeds_embeddings() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        // Minimal v12-era fixture: meetings v3 shape with structured_summary,
        // segments referencing one of two speakers, and the v12 speakers
        // table (with a centroid) but no speaker_embeddings table yet.
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(CREATE_SCHEMA_VERSION).unwrap();
        conn.execute_batch(super::CREATE_MEETINGS_V3).unwrap();
        conn.execute(
            "ALTER TABLE meetings ADD COLUMN structured_summary TEXT",
            [],
        )
        .unwrap();
        conn.execute_batch(CREATE_SEGMENTS).unwrap();
        conn.execute_batch(CREATE_SEGMENTS_INDEX).unwrap();
        // A real v12 database has already been through the v7 migration.
        conn.execute("ALTER TABLE segments ADD COLUMN speaker TEXT", [])
            .unwrap();
        conn.execute_batch(CREATE_DICTATION_ENTRIES).unwrap();
        conn.execute_batch(CREATE_SETTINGS).unwrap();
        conn.execute_batch(super::CREATE_TEXT_SEARCH).unwrap();
        conn.execute_batch(super::CREATE_SPEAKERS).unwrap();
        conn.execute("INSERT INTO schema_version (version) VALUES (12)", [])
            .unwrap();

        conn.execute(
            "INSERT INTO meetings (id, title, started_at, ended_at, duration_seconds, transcription_profile, recording_sessions, summary, summary_is_stale, summary_model, summary_generated_at, edited_transcript)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                "m1",
                "Meeting",
                "2026-03-01T10:00:00Z",
                "2026-03-01T10:10:00Z",
                600.0,
                r#"{"engine_id":"parakeet","engine_label":"Parakeet","model_id":"parakeet-tdt-0.6b-v2","model_label":"Parakeet TDT 0.6B v2","backend_id":"ort","backend_label":"ONNX Runtime"}"#,
                "[]",
                None::<String>,
                0,
                None::<String>,
                None::<String>,
                None::<String>,
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO segments (meeting_id, text, start_time, end_time, is_final, language, confidence, sort_order, speaker)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params!["m1", "Hello", 0.0, 1.0, 1, Some("en"), None::<f32>, 0, "spk:1"],
        )
        .unwrap();

        let now = "2026-03-01T10:00:00Z";
        // Speaker 1 is referenced by a segment and has a centroid: it must
        // survive and get that centroid seeded as an embedding.
        conn.execute(
            "INSERT INTO speakers (id, name, centroid, embedding_count, created_at, last_seen_at) VALUES (1, 'Alice', ?1, 3, ?2, ?2)",
            params![vec![0u8; 1024], now],
        )
        .unwrap();
        // Speaker 2 has no segment referencing it: orphaned, must be purged.
        conn.execute(
            "INSERT INTO speakers (id, name, centroid, embedding_count, created_at, last_seen_at) VALUES (2, 'Orphan', NULL, 0, ?1, ?1)",
            params![now],
        )
        .unwrap();
        drop(conn);

        let db = Database::open(&db_path).unwrap();
        let conn = db.conn.lock().unwrap();

        let speaker_ids: Vec<i64> = {
            let mut stmt = conn.prepare("SELECT id FROM speakers ORDER BY id").unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            speaker_ids,
            vec![1],
            "orphaned speaker 2 must be purged, referenced speaker 1 kept"
        );

        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='speaker_embeddings'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_exists, "v13 migration should create speaker_embeddings");

        let seeded_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM speaker_embeddings WHERE speaker_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            seeded_count, 1,
            "surviving speaker with a centroid should get one seeded embedding"
        );

        let is_me: i64 = conn
            .query_row("SELECT is_me FROM speakers WHERE id = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(is_me, 0, "is_me should default to 0");
    }

    #[test]
    fn v12_speakers_without_is_me_column_migrate_to_v13_add_it() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        // A real v12 database predating this column: the speakers table has
        // the pre-v13 shape (no is_me), unlike super::CREATE_SPEAKERS which
        // already includes it.
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(CREATE_SCHEMA_VERSION).unwrap();
        conn.execute_batch(super::CREATE_MEETINGS_V3).unwrap();
        conn.execute(
            "ALTER TABLE meetings ADD COLUMN structured_summary TEXT",
            [],
        )
        .unwrap();
        conn.execute_batch(CREATE_SEGMENTS).unwrap();
        conn.execute_batch(CREATE_SEGMENTS_INDEX).unwrap();
        conn.execute("ALTER TABLE segments ADD COLUMN speaker TEXT", [])
            .unwrap();
        conn.execute_batch(CREATE_DICTATION_ENTRIES).unwrap();
        conn.execute_batch(CREATE_SETTINGS).unwrap();
        conn.execute_batch(super::CREATE_TEXT_SEARCH).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS speakers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                centroid BLOB,
                embedding_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                last_seen_at TEXT NOT NULL
            );",
        )
        .unwrap();
        conn.execute("INSERT INTO schema_version (version) VALUES (12)", [])
            .unwrap();

        let now = "2026-03-01T10:00:00Z";
        conn.execute(
            "INSERT INTO speakers (id, name, centroid, embedding_count, created_at, last_seen_at) VALUES (1, 'Alice', NULL, 0, ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO meetings (id, title, started_at, ended_at, duration_seconds, transcription_profile, recording_sessions, summary, summary_is_stale, summary_model, summary_generated_at, edited_transcript)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                "m1",
                "Meeting",
                now,
                now,
                600.0,
                r#"{"engine_id":"parakeet","engine_label":"Parakeet","model_id":"parakeet-tdt-0.6b-v2","model_label":"Parakeet TDT 0.6B v2","backend_id":"ort","backend_label":"ONNX Runtime"}"#,
                "[]",
                None::<String>,
                0,
                None::<String>,
                None::<String>,
                None::<String>,
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO segments (meeting_id, text, start_time, end_time, is_final, language, confidence, sort_order, speaker)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params!["m1", "Hello", 0.0, 1.0, 1, Some("en"), None::<f32>, 0, "spk:1"],
        )
        .unwrap();
        drop(conn);

        let db = Database::open(&db_path).unwrap();
        let conn = db.conn.lock().unwrap();

        let columns: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(speakers)").unwrap();
            stmt.query_map([], |row| row.get(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert!(
            columns.iter().any(|column| column == "is_me"),
            "v13 migration should add is_me to a v12 speakers table missing it"
        );

        let is_me: i64 = conn
            .query_row("SELECT is_me FROM speakers WHERE id = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(is_me, 0, "is_me should default to 0 for existing rows");
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
