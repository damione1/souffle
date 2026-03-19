/// Schema version 1: meetings, segments, dictation, settings, FTS5, embeddings
pub const SCHEMA_VERSION: i64 = 1;

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
