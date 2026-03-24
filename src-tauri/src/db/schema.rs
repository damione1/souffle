/// Schema version 2: typed transcription profile metadata for meetings
pub const SCHEMA_VERSION: i64 = 2;

pub const CREATE_SCHEMA_VERSION: &str = "
    CREATE TABLE IF NOT EXISTS schema_version (
        version INTEGER NOT NULL
    );
";

pub const CREATE_MEETINGS: &str = "
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

pub const CREATE_TEXT_SEARCH: &str = "
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

/// All schema creation statements in order
pub const SCHEMA_V1: &[&str] = &[
    CREATE_SCHEMA_VERSION,
    CREATE_MEETINGS,
    CREATE_SEGMENTS,
    CREATE_SEGMENTS_INDEX,
    CREATE_DICTATION_ENTRIES,
    CREATE_SETTINGS,
    CREATE_TEXT_SEARCH,
    CREATE_EMBEDDINGS,
    CREATE_EMBEDDINGS_INDEX,
];

pub const SCHEMA_V2: &[&str] = &["ALTER TABLE meetings ADD COLUMN transcription_profile TEXT;"];

pub const MIGRATIONS: &[(i64, &[&str])] = &[(1, SCHEMA_V1), (2, SCHEMA_V2)];

#[cfg(test)]
mod tests {
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
}
