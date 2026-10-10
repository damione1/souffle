//! Identifiers on the app/MCP dual-reader surface. Compose SQL from these
//! constants; bind all data values separately. Local table/column literals
//! are forbidden in live queries and current CREATE statements. Historical
//! migrations keep their original identifiers, explicitly marked as snapshots.

pub mod table {
    pub const SCHEMA_VERSION: &str = "schema_version";
    pub const MEETINGS: &str = "meetings";
    pub const SEGMENTS: &str = "segments";
    pub const DICTATION_ENTRIES: &str = "dictation_entries";
    pub const TEXT_SEARCH: &str = "text_search";
}

/// A spelling shared by multiple tables (e.g. `id` or `text`) is declared once.
pub mod column {
    pub const VERSION: &str = "version";
    pub const ID: &str = "id";
    pub const TITLE: &str = "title";
    pub const STARTED_AT: &str = "started_at";
    pub const ENDED_AT: &str = "ended_at";
    pub const DURATION_SECONDS: &str = "duration_seconds";
    pub const TRANSCRIPTION_PROFILE: &str = "transcription_profile";
    pub const RECORDING_SESSIONS: &str = "recording_sessions";
    pub const SUMMARY: &str = "summary";
    pub const SUMMARY_IS_STALE: &str = "summary_is_stale";
    pub const SUMMARY_MODEL: &str = "summary_model";
    pub const SUMMARY_GENERATED_AT: &str = "summary_generated_at";
    pub const EDITED_TRANSCRIPT: &str = "edited_transcript";
    pub const NOTES: &str = "notes";
    pub const CALENDAR_EVENT_ID: &str = "calendar_event_id";
    pub const PARTICIPANTS: &str = "participants";
    pub const STRUCTURED_SUMMARY: &str = "structured_summary";
    pub const SYSTEM_AUDIO: &str = "system_audio";
    pub const MEETING_ID: &str = "meeting_id";
    pub const TEXT: &str = "text";
    pub const START_TIME: &str = "start_time";
    pub const END_TIME: &str = "end_time";
    pub const IS_FINAL: &str = "is_final";
    pub const LANGUAGE: &str = "language";
    pub const CONFIDENCE: &str = "confidence";
    pub const SORT_ORDER: &str = "sort_order";
    pub const SPEAKER: &str = "speaker";
    pub const TIMESTAMP: &str = "timestamp";
    pub const CONTENT: &str = "content";
    pub const SOURCE_TYPE: &str = "source_type";
    pub const SOURCE_ID: &str = "source_id";
    /// FTS5's hidden ranking column, used by both readers.
    pub const RANK: &str = "rank";
}
