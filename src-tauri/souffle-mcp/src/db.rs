//! Read-only data layer for the MCP sidecar.
//!
//! This intentionally does not depend on the app crate: it opens the same
//! SQLite file the app writes to (`SQLITE_OPEN_READ_ONLY`, so it can run
//! concurrently with the app under WAL) and re-implements just the read
//! queries the MCP tools need. Keeping the two crates independent is the
//! whole point of the sidecar (it must build and run without pulling in
//! Tauri, candle, or ort), at the cost of the schema being duplicated on the
//! read side. `souffle-mcp` schema drift against the writer is caught by the
//! contract test in `src-tauri/tests/mcp_sidecar_contract.rs`, which writes
//! through the real app `Database` and reads back through this module.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Must match `constants::APP_IDENTIFIER` in the main crate.
const APP_IDENTIFIER: &str = "com.souffle.desktop";

/// Gap between segments, in seconds, that starts a new paragraph in the
/// simplified transcript renderer below.
const PARAGRAPH_GAP_SECONDS: f64 = 1.5;

#[derive(Debug, Error)]
pub enum McpDbError {
    #[error(
        "Souffle database not found at {0}. Launch Souffle at least once so it can create it."
    )]
    NotFound(PathBuf),
    #[error("Open database: {0}")]
    Open(#[source] rusqlite::Error),
    #[error("Query failed: {0}")]
    Query(#[source] rusqlite::Error),
    #[error("Meeting not found: {0}")]
    MeetingNotFound(String),
    #[error("No meetings found")]
    NoMeetings,
    #[error("Database lock poisoned: {0}")]
    Lock(String),
}

/// Resolve the Souffle SQLite database path: the `SOUFFLE_DB` env var
/// overrides everything (used by tests and manual debugging); otherwise this
/// mirrors `constants::app_data_dir()` in the main crate.
pub fn resolve_db_path() -> PathBuf {
    if let Ok(path) = std::env::var("SOUFFLE_DB") {
        return PathBuf::from(path);
    }
    dirs_next::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_IDENTIFIER)
        .join("souffle.db")
}

#[derive(Debug, Clone, Copy)]
pub struct IncludeSet {
    pub transcript: bool,
    pub summary: bool,
    pub notes: bool,
    pub metadata: bool,
}

impl IncludeSet {
    pub fn all() -> Self {
        Self {
            transcript: true,
            summary: true,
            notes: true,
            metadata: true,
        }
    }

    /// `None` or an empty list both mean "everything" — an MCP client asking
    /// for a meeting with no `include` filter almost always wants the full
    /// picture, not nothing.
    pub fn from_names(names: Option<&[String]>) -> Self {
        let Some(names) = names else {
            return Self::all();
        };
        if names.is_empty() {
            return Self::all();
        }
        Self {
            transcript: names.iter().any(|n| n == "transcript"),
            summary: names.iter().any(|n| n == "summary"),
            notes: names.iter().any(|n| n == "notes"),
            metadata: names.iter().any(|n| n == "metadata"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ParticipantInfo {
    pub name: String,
    pub email: Option<String>,
    pub is_organizer: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StructuredActionItem {
    pub text: String,
    pub owner: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct StructuredSummary {
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub action_items: Vec<StructuredActionItem>,
    #[serde(default)]
    pub open_questions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct MeetingSummary {
    pub id: String,
    pub title: String,
    pub started_at: String,
    pub duration_seconds: f64,
    pub participants: Vec<String>,
    pub has_summary: bool,
    pub has_notes: bool,
}

/// A persistent speaker referenced by a meeting's segments, resolved for
/// display. Mirrors the app crate's `MeetingSpeaker` DTO (kept independent,
/// see the module doc comment); empty for Me/Them-only meetings.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct MeetingSpeakerInfo {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct MeetingMetadata {
    pub calendar_event_id: Option<String>,
    pub participants: Vec<ParticipantInfo>,
    pub summary_model: Option<String>,
    pub summary_generated_at: Option<String>,
    pub segment_count: usize,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct MeetingDetail {
    pub id: String,
    pub title: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub duration_seconds: f64,
    pub transcript: Option<String>,
    pub summary: Option<String>,
    pub structured_summary: Option<StructuredSummary>,
    pub notes: Option<String>,
    pub metadata: Option<MeetingMetadata>,
    /// Persistent speakers referenced by this meeting's segments, for
    /// resolving `spk:<id>` labels in `transcript`. Empty for Me/Them-only
    /// meetings, or when neither `transcript` nor `metadata` was requested.
    pub speakers: Vec<MeetingSpeakerInfo>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct MeetingSearchHit {
    pub id: String,
    pub title: String,
    pub started_at: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DictationSummary {
    pub id: String,
    pub text: String,
    pub timestamp: String,
}

/// Raw `segments` row from SQLite; diarized meetings are re-sorted by time
/// before rendering (see `time_ordered_segments`).
struct SegmentRow {
    text: String,
    start_time: f64,
    end_time: f64,
    speaker: Option<String>,
}

struct MeetingRow {
    id: String,
    title: String,
    started_at: String,
    ended_at: Option<String>,
    duration_seconds: f64,
    summary: Option<String>,
    summary_model: Option<String>,
    summary_generated_at: Option<String>,
    structured_summary: Option<String>,
    edited_transcript: Option<String>,
    notes: Option<String>,
    calendar_event_id: Option<String>,
    participants: Option<String>,
}

impl MeetingRow {
    /// Malformed participant JSON is treated as "no participants" rather
    /// than failing the whole meeting read — it is a cosmetic field.
    fn participants(&self) -> Vec<ParticipantInfo> {
        self.participants
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or_default()
    }

    fn participant_names(&self) -> Vec<String> {
        self.participants()
            .into_iter()
            .map(|p| p.name)
            .collect()
    }

    fn structured_summary(&self) -> Option<StructuredSummary> {
        self.structured_summary
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok())
    }
}

#[derive(Debug)]
pub struct McpDb {
    /// `Mutex` (not just `Connection`) so `McpDb` is `Sync` — the rmcp tool
    /// macros generate `Send` futures that hold `&SouffleMcpServer` across
    /// await points, and `rusqlite::Connection` alone is `!Sync`.
    conn: Mutex<Connection>,
}

impl McpDb {
    /// Open the database read-only. WAL mode lets this coexist with the app
    /// writing concurrently; `busy_timeout` covers the rare case where a
    /// writer holds a brief exclusive lock during checkpointing.
    pub fn open(path: &Path) -> Result<Self, McpDbError> {
        if !path.exists() {
            return Err(McpDbError::NotFound(path.to_path_buf()));
        }
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(McpDbError::Open)?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(McpDbError::Open)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, McpDbError> {
        self.conn
            .lock()
            .map_err(|e| McpDbError::Lock(e.to_string()))
    }

    pub fn list_meetings(
        &self,
        query: Option<&str>,
        from: Option<&str>,
        to: Option<&str>,
        limit: i64,
    ) -> Result<Vec<MeetingSummary>, McpDbError> {
        let conn = self.conn()?;
        let rows = if let Some(q) = query.filter(|q| !q.trim().is_empty()) {
            let mut stmt = conn
                .prepare(
                    "SELECT m.id, m.title, m.started_at, m.ended_at, m.duration_seconds,
                            m.summary, m.summary_model, m.summary_generated_at,
                            m.structured_summary,
                            m.edited_transcript, m.notes, m.calendar_event_id, m.participants
                     FROM meetings m
                     WHERE m.id IN (
                         SELECT source_id FROM text_search
                         WHERE source_type = 'meeting' AND text_search MATCH ?1
                     )
                     AND (?2 IS NULL OR julianday(m.started_at) >= julianday(?2))
                     AND (?3 IS NULL OR julianday(m.started_at) <= julianday(?3))
                     ORDER BY m.started_at DESC
                     LIMIT ?4",
                )
                .map_err(McpDbError::Query)?;
            query_meeting_rows(&mut stmt, params![q, from, to, limit])?
        } else {
            let mut stmt = conn
                .prepare(
                    "SELECT m.id, m.title, m.started_at, m.ended_at, m.duration_seconds,
                            m.summary, m.summary_model, m.summary_generated_at,
                            m.structured_summary,
                            m.edited_transcript, m.notes, m.calendar_event_id, m.participants
                     FROM meetings m
                     WHERE (?1 IS NULL OR julianday(m.started_at) >= julianday(?1))
                       AND (?2 IS NULL OR julianday(m.started_at) <= julianday(?2))
                     ORDER BY m.started_at DESC
                     LIMIT ?3",
                )
                .map_err(McpDbError::Query)?;
            query_meeting_rows(&mut stmt, params![from, to, limit])?
        };

        Ok(rows
            .into_iter()
            .map(|row| MeetingSummary {
                has_summary: row.summary.is_some(),
                has_notes: row.notes.as_deref().is_some_and(|n| !n.is_empty()),
                participants: row.participant_names(),
                id: row.id,
                title: row.title,
                started_at: row.started_at,
                duration_seconds: row.duration_seconds,
            })
            .collect())
    }

    pub fn get_meeting(&self, id: &str, include: IncludeSet) -> Result<MeetingDetail, McpDbError> {
        let row = self
            .conn()?
            .query_row(
                "SELECT id, title, started_at, ended_at, duration_seconds,
                        summary, summary_model, summary_generated_at, structured_summary,
                        edited_transcript, notes, calendar_event_id, participants
                 FROM meetings WHERE id = ?1",
                params![id],
                map_meeting_row,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => McpDbError::MeetingNotFound(id.to_string()),
                other => McpDbError::Query(other),
            })?;

        self.build_meeting_detail(row, include)
    }

    pub fn latest_meeting(&self, include: IncludeSet) -> Result<MeetingDetail, McpDbError> {
        let id: Option<String> = self
            .conn()?
            .query_row(
                "SELECT id FROM meetings ORDER BY started_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => McpDbError::NoMeetings,
                other => McpDbError::Query(other),
            })?;
        let id = id.ok_or(McpDbError::NoMeetings)?;
        self.get_meeting(&id, include)
    }

    fn build_meeting_detail(
        &self,
        row: MeetingRow,
        include: IncludeSet,
    ) -> Result<MeetingDetail, McpDbError> {
        let need_segments = include.transcript || include.metadata;
        let segments = if need_segments {
            self.load_segments(&row.id)?
        } else {
            Vec::new()
        };
        let speakers = if need_segments {
            self.speakers_for_meeting(&row.id)?
        } else {
            Vec::new()
        };

        let transcript = if include.transcript {
            Some(
                row.edited_transcript
                    .clone()
                    .unwrap_or_else(|| render_transcript(&segments, &speakers)),
            )
        } else {
            None
        };

        let summary = if include.summary { row.summary.clone() } else { None };
        let structured_summary = if include.summary {
            row.structured_summary()
        } else {
            None
        };
        let notes = if include.notes { row.notes.clone() } else { None };
        let metadata = if include.metadata {
            Some(MeetingMetadata {
                calendar_event_id: row.calendar_event_id.clone(),
                participants: row.participants(),
                summary_model: row.summary_model.clone(),
                summary_generated_at: row.summary_generated_at.clone(),
                segment_count: segments.len(),
            })
        } else {
            None
        };

        Ok(MeetingDetail {
            id: row.id,
            title: row.title,
            started_at: row.started_at,
            ended_at: row.ended_at,
            duration_seconds: row.duration_seconds,
            transcript,
            summary,
            structured_summary,
            notes,
            metadata,
            speakers,
        })
    }

    /// Persistent speakers referenced by `meeting_id`'s segments (`spk:<id>`
    /// labels), resolved against the `speakers` table. Mirrors the app
    /// crate's `Database::speakers_for_meeting`.
    fn speakers_for_meeting(&self, meeting_id: &str) -> Result<Vec<MeetingSpeakerInfo>, McpDbError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT speaker FROM segments
                 WHERE meeting_id = ?1 AND speaker LIKE 'spk:%'",
            )
            .map_err(McpDbError::Query)?;
        let ids: Vec<i64> = stmt
            .query_map(params![meeting_id], |row| row.get::<_, String>(0))
            .map_err(McpDbError::Query)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(McpDbError::Query)?
            .into_iter()
            .filter_map(|raw| raw.strip_prefix("spk:")?.parse::<i64>().ok())
            .collect();

        let mut speakers = Vec::with_capacity(ids.len());
        for id in ids {
            let name: Option<String> = conn
                .query_row("SELECT name FROM speakers WHERE id = ?1", params![id], |row| {
                    row.get(0)
                })
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(McpDbError::Query(other)),
                })?;
            if let Some(name) = name {
                speakers.push(MeetingSpeakerInfo { id, name });
            }
        }
        speakers.sort_by_key(|s| s.id);
        Ok(speakers)
    }

    fn load_segments(&self, meeting_id: &str) -> Result<Vec<SegmentRow>, McpDbError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT text, start_time, end_time, speaker
                 FROM segments WHERE meeting_id = ?1 ORDER BY sort_order",
            )
            .map_err(McpDbError::Query)?;

        let segments = stmt
            .query_map(params![meeting_id], |row| {
                Ok(SegmentRow {
                    text: row.get(0)?,
                    start_time: row.get(1)?,
                    end_time: row.get(2)?,
                    speaker: row.get(3)?,
                })
            })
            .map_err(McpDbError::Query)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(McpDbError::Query)?;
        Ok(time_ordered_segments(segments))
    }

    pub fn search_meetings(
        &self,
        query: &str,
        limit: i64,
    ) -> Result<Vec<MeetingSearchHit>, McpDbError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT ts.source_id, m.title, m.started_at,
                        snippet(text_search, 0, '**', '**', '...', 32)
                 FROM text_search ts
                 JOIN meetings m ON m.id = ts.source_id
                 WHERE ts.source_type = 'meeting' AND text_search MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )
            .map_err(McpDbError::Query)?;

        stmt.query_map(params![query, limit], |row| {
            Ok(MeetingSearchHit {
                id: row.get(0)?,
                title: row.get(1)?,
                started_at: row.get(2)?,
                snippet: row.get(3)?,
            })
        })
        .map_err(McpDbError::Query)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(McpDbError::Query)
    }

    pub fn list_dictations(&self, limit: i64) -> Result<Vec<DictationSummary>, McpDbError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, text, timestamp FROM dictation_entries
                 ORDER BY timestamp DESC LIMIT ?1",
            )
            .map_err(McpDbError::Query)?;

        stmt.query_map(params![limit], |row| {
            Ok(DictationSummary {
                id: row.get(0)?,
                text: row.get(1)?,
                timestamp: row.get(2)?,
            })
        })
        .map_err(McpDbError::Query)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(McpDbError::Query)
    }
}

fn map_meeting_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MeetingRow> {
    Ok(MeetingRow {
        id: row.get(0)?,
        title: row.get(1)?,
        started_at: row.get(2)?,
        ended_at: row.get(3)?,
        duration_seconds: row.get(4)?,
        summary: row.get(5)?,
        summary_model: row.get(6)?,
        summary_generated_at: row.get(7)?,
        structured_summary: row.get(8)?,
        edited_transcript: row.get(9)?,
        notes: row.get(10)?,
        calendar_event_id: row.get(11)?,
        participants: row.get(12)?,
    })
}

/// Diarized meetings interleave Me/Them segments by processing frame in
/// storage order, not strictly by time — mirror export's
/// `time_ordered_segments` so MCP transcript text reads as a conversation.
fn time_ordered_segments(mut segments: Vec<SegmentRow>) -> Vec<SegmentRow> {
    let diarized = segments.iter().any(|s| s.speaker.is_some());
    if diarized {
        segments.sort_by(|a, b| {
            a.start_time
                .partial_cmp(&b.start_time)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.speaker.cmp(&b.speaker))
        });
    }
    segments
}

fn query_meeting_rows(
    stmt: &mut rusqlite::Statement<'_>,
    params: impl rusqlite::Params,
) -> Result<Vec<MeetingRow>, McpDbError> {
    stmt.query_map(params, map_meeting_row)
        .map_err(McpDbError::Query)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(McpDbError::Query)
}

/// Sentence-ending punctuation, allowing closing quotes/brackets after it.
/// Simplified mirror of the TS `SENTENCE_END` regex
/// (`/[.!?…]["»”')\]]*\s*$/`), only used here to decide when an interrupted
/// turn (see `cluster_into_turns`) should close.
fn ends_sentence(text: &str) -> bool {
    const CLOSING_CHARS: [char; 6] = ['"', '\u{00bb}', '\u{201d}', '\'', ')', ']'];
    const SENTENCE_END_CHARS: [char; 4] = ['.', '!', '?', '\u{2026}'];

    let mut chars: Vec<char> = text.trim_end().chars().collect();
    while matches!(chars.last(), Some(c) if CLOSING_CHARS.contains(c)) {
        chars.pop();
    }
    matches!(chars.last(), Some(c) if SENTENCE_END_CHARS.contains(c))
}

/// Cluster time-ordered segments into per-speaker turns, then emit them
/// ordered by each turn's start time (turn segments stay contiguous and
/// chronological internally). A segment joins its speaker's currently open
/// turn if the gap since that turn's last segment is under
/// `PARAGRAPH_GAP_SECONDS`; otherwise that speaker's turn closes and a new
/// one opens. Simplified mirror of `clusterIntoTurns` in
/// `src/lib/utils/paragraphs.ts`, kept a stand-in rather than a faithful
/// port: same idea (during crosstalk, keep each speaker's words together
/// instead of breaking on every interleaved segment), no sentence/length
/// heuristics beyond the interruption rule below.
///
/// Without a pause, a monologue would otherwise absorb everything
/// indefinitely, so a second speaker's interjection would render far below
/// the point in the monologue it actually responded to. To keep
/// interjections anchored near their moment: opening a new turn marks every
/// other speaker's currently open turn as interrupted. An interrupted turn
/// still keeps absorbing segments, but closes as soon as one of them ends a
/// sentence, so the speaker's next segment opens a fresh turn that sorts
/// after the interjection. A turn that never hits a sentence end (no
/// punctuation) still only closes on pause, same as before.
fn cluster_into_turns(segments: &[SegmentRow]) -> Vec<&SegmentRow> {
    struct Turn<'a> {
        start: f64,
        last_end: f64,
        segments: Vec<&'a SegmentRow>,
        interrupted: bool,
    }

    let mut open: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut turns: Vec<Turn> = Vec::new();

    for seg in segments {
        let end = seg.end_time.max(seg.start_time);
        let Some(speaker) = seg.speaker.as_deref() else {
            turns.push(Turn { start: seg.start_time, last_end: end, segments: vec![seg], interrupted: false });
            continue;
        };

        let joins = open
            .get(speaker)
            .is_some_and(|&idx| seg.start_time - turns[idx].last_end < PARAGRAPH_GAP_SECONDS);

        if joins {
            let idx = open[speaker];
            turns[idx].segments.push(seg);
            turns[idx].last_end = turns[idx].last_end.max(end);
            if turns[idx].interrupted && ends_sentence(seg.text.trim()) {
                // First sentence end at or after the interruption: close now
                // so the speaker's next segment starts a fresh, later-sorting
                // turn.
                open.remove(speaker);
            }
        } else {
            turns.push(Turn { start: seg.start_time, last_end: end, segments: vec![seg], interrupted: false });
            open.insert(speaker, turns.len() - 1);
            for (&other_speaker, &idx) in open.iter() {
                if other_speaker != speaker {
                    turns[idx].interrupted = true;
                }
            }
        }
    }

    turns.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    turns.into_iter().flat_map(|t| t.segments).collect()
}

/// Display label for a raw `segments.speaker` value: "me" -> "Me", "them" ->
/// "Them", "spk:<id>" -> the matching `MeetingSpeakerInfo.name` if resolvable,
/// else a "Speaker <id>" fallback. An unrecognized raw value (shouldn't
/// happen) passes through unchanged.
fn speaker_label(raw: &str, speakers: &[MeetingSpeakerInfo]) -> String {
    match raw {
        "me" => "Me".to_string(),
        "them" => "Them".to_string(),
        other => match other.strip_prefix("spk:").and_then(|id| id.parse::<i64>().ok()) {
            Some(id) => speakers
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| format!("Speaker {id}")),
            None => other.to_string(),
        },
    }
}

/// Simplified stand-in for the frontend's paragraph engine
/// (`src/lib/utils/paragraphs.ts`): breaks on every speaker/turn change and
/// on gaps over `PARAGRAPH_GAP_SECONDS`, prefixing each new paragraph with
/// the speaker label when known. Diarized input is first clustered into
/// per-speaker turns (see `cluster_into_turns`) so crosstalk reads as
/// flowing per-speaker phrases rather than breaking on every interleaved
/// segment. It does not replicate the sentence/length heuristics the app
/// uses for on-screen readability: this is meant to be a faithful, readable
/// text dump for an AI assistant, not pixel-identical to the app's
/// transcript view.
fn render_transcript(segments: &[SegmentRow], speakers: &[MeetingSpeakerInfo]) -> String {
    let diarized = segments.iter().any(|s| s.speaker.is_some());
    let ordered: Vec<&SegmentRow> = if diarized {
        cluster_into_turns(segments)
    } else {
        segments.iter().collect()
    };

    let mut paragraphs: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_speaker: Option<String> = None;
    let mut last_end = 0.0_f64;
    let mut started = false;

    for seg in ordered {
        let text = seg.text.trim();
        if text.is_empty() {
            continue;
        }

        let speaker_changed = started && seg.speaker != current_speaker;
        let big_gap = started && (seg.start_time - last_end) > PARAGRAPH_GAP_SECONDS;

        if (speaker_changed || big_gap) && !current.is_empty() {
            paragraphs.push(std::mem::take(&mut current));
        }

        if current.is_empty() {
            if let Some(speaker) = &seg.speaker {
                current.push_str(&speaker_label(speaker, speakers));
                current.push_str(": ");
            }
        } else {
            current.push(' ');
        }
        current.push_str(text);

        current_speaker = seg.speaker.clone();
        last_end = seg.end_time.max(seg.start_time);
        started = true;
    }

    if !current.is_empty() {
        paragraphs.push(current);
    }

    paragraphs.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::TempDir;

    /// Minimal fixture mirroring the app's current schema (meetings v10 +
    /// segments + dictation_entries + text_search FTS5). Kept intentionally
    /// small: the schema-drift contract test in
    /// `src-tauri/tests/mcp_sidecar_contract.rs` is what actually guards
    /// this against the real writer.
    fn fixture_db() -> (Connection, TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("fixture.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "
            CREATE TABLE meetings (
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
                structured_summary TEXT,
                edited_transcript TEXT,
                notes TEXT,
                calendar_event_id TEXT,
                participants TEXT
            );
            CREATE TABLE segments (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                meeting_id TEXT NOT NULL,
                text TEXT NOT NULL,
                start_time REAL NOT NULL,
                end_time REAL NOT NULL,
                is_final INTEGER NOT NULL DEFAULT 1,
                language TEXT,
                confidence REAL,
                sort_order INTEGER NOT NULL,
                speaker TEXT
            );
            CREATE TABLE dictation_entries (
                id TEXT PRIMARY KEY,
                text TEXT NOT NULL,
                timestamp TEXT NOT NULL
            );
            CREATE VIRTUAL TABLE text_search USING fts5(
                content, source_type, source_id
            );
            CREATE TABLE speakers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                centroid BLOB,
                embedding_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                last_seen_at TEXT NOT NULL
            );
            ",
        )
        .unwrap();
        (conn, dir, path)
    }

    fn insert_speaker(conn: &Connection, id: i64, name: &str) {
        conn.execute(
            "INSERT INTO speakers (id, name, embedding_count, created_at, last_seen_at)
             VALUES (?1, ?2, 0, '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')",
            params![id, name],
        )
        .unwrap();
    }

    fn insert_meeting(
        conn: &Connection,
        id: &str,
        title: &str,
        started_at: &str,
        segments: &[(&str, f64, f64, Option<&str>)],
        participants: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO meetings (id, title, started_at, ended_at, duration_seconds, transcription_profile, recording_sessions, participants)
             VALUES (?1, ?2, ?3, ?3, 10.0, '{}', '[]', ?4)",
            params![id, title, started_at, participants],
        )
        .unwrap();

        let mut full_text = Vec::new();
        for (i, (text, start, end, speaker)) in segments.iter().enumerate() {
            conn.execute(
                "INSERT INTO segments (meeting_id, text, start_time, end_time, sort_order, speaker)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, text, start, end, i as i64, speaker],
            )
            .unwrap();
            full_text.push(*text);
        }
        if !full_text.is_empty() {
            conn.execute(
                "INSERT INTO text_search (content, source_type, source_id) VALUES (?1, 'meeting', ?2)",
                params![full_text.join(" "), id],
            )
            .unwrap();
        }
    }

    fn insert_dictation(conn: &Connection, id: &str, text: &str, timestamp: &str) {
        conn.execute(
            "INSERT INTO dictation_entries (id, text, timestamp) VALUES (?1, ?2, ?3)",
            params![id, text, timestamp],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO text_search (content, source_type, source_id) VALUES (?1, 'dictation', ?2)",
            params![text, id],
        )
        .unwrap();
    }

    #[test]
    fn missing_db_returns_clean_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nope.db");
        let err = McpDb::open(&path).unwrap_err();
        assert!(matches!(err, McpDbError::NotFound(_)));
        assert!(err.to_string().contains("Launch Souffle"));
    }

    #[test]
    fn list_meetings_orders_newest_first_and_respects_limit() {
        let (conn, _dir, path) = fixture_db();
        insert_meeting(&conn, "m1", "First", "2026-01-01T10:00:00+00:00", &[("hi", 0.0, 1.0, None)], None);
        insert_meeting(&conn, "m2", "Second", "2026-01-02T10:00:00+00:00", &[("hi", 0.0, 1.0, None)], None);
        insert_meeting(&conn, "m3", "Third", "2026-01-03T10:00:00+00:00", &[("hi", 0.0, 1.0, None)], None);
        drop(conn);

        let db = McpDb::open(&path).unwrap();
        let all = db.list_meetings(None, None, None, 20).unwrap();
        assert_eq!(all.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["m3", "m2", "m1"]);

        let limited = db.list_meetings(None, None, None, 2).unwrap();
        assert_eq!(limited.len(), 2);
    }

    #[test]
    fn list_meetings_filters_by_date_range() {
        let (conn, _dir, path) = fixture_db();
        insert_meeting(&conn, "m1", "First", "2026-01-01T10:00:00+00:00", &[("hi", 0.0, 1.0, None)], None);
        insert_meeting(&conn, "m2", "Second", "2026-02-01T10:00:00+00:00", &[("hi", 0.0, 1.0, None)], None);
        drop(conn);

        let db = McpDb::open(&path).unwrap();
        let filtered = db
            .list_meetings(None, Some("2026-01-15"), None, 20)
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "m2");
    }

    #[test]
    fn list_meetings_filters_by_query() {
        let (conn, _dir, path) = fixture_db();
        insert_meeting(&conn, "m1", "First", "2026-01-01T10:00:00+00:00", &[("budget review", 0.0, 1.0, None)], None);
        insert_meeting(&conn, "m2", "Second", "2026-01-02T10:00:00+00:00", &[("standup notes", 0.0, 1.0, None)], None);
        drop(conn);

        let db = McpDb::open(&path).unwrap();
        let hits = db.list_meetings(Some("budget"), None, None, 20).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "m1");
    }

    #[test]
    fn list_meetings_reports_participant_names_and_flags() {
        let (conn, _dir, path) = fixture_db();
        conn.execute(
            "INSERT INTO meetings (id, title, started_at, duration_seconds, transcription_profile, recording_sessions, summary, notes, participants)
             VALUES ('m1', 'Standup', '2026-01-01T10:00:00+00:00', 10.0, '{}', '[]', 'a summary', 'some notes',
                     '[{\"name\":\"Alice\",\"email\":null,\"is_organizer\":true}]')",
            [],
        )
        .unwrap();
        drop(conn);

        let db = McpDb::open(&path).unwrap();
        let items = db.list_meetings(None, None, None, 20).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].participants, vec!["Alice".to_string()]);
        assert!(items[0].has_summary);
        assert!(items[0].has_notes);
    }

    #[test]
    fn get_meeting_orders_interleaved_diarized_segments_by_start_time() {
        let (conn, _dir, path) = fixture_db();
        insert_meeting(
            &conn,
            "m1",
            "Standup",
            "2026-01-01T10:00:00+00:00",
            &[
                ("Second line", 5.0, 6.0, Some("them")),
                ("First line", 1.0, 2.0, Some("me")),
                ("Third line", 8.0, 9.0, Some("me")),
            ],
            None,
        );
        drop(conn);

        let db = McpDb::open(&path).unwrap();
        let transcript = db.get_meeting("m1", IncludeSet::all()).unwrap().transcript.unwrap();
        let me_pos = transcript.find("Me: First line").expect("me segment");
        let them_pos = transcript.find("Them: Second line").expect("them segment");
        let me2_pos = transcript.find("Me: Third line").expect("me segment 2");
        assert!(me_pos < them_pos);
        assert!(them_pos < me2_pos);
    }

    #[test]
    fn get_meeting_renders_transcript_from_segments_when_not_edited() {
        let (conn, _dir, path) = fixture_db();
        insert_meeting(
            &conn,
            "m1",
            "Standup",
            "2026-01-01T10:00:00+00:00",
            &[
                ("Hello there.", 0.0, 1.0, Some("me")),
                ("General Kenobi.", 1.2, 2.0, Some("them")),
                ("Much later.", 30.0, 31.0, Some("them")),
            ],
            None,
        );
        drop(conn);

        let db = McpDb::open(&path).unwrap();
        let detail = db.get_meeting("m1", IncludeSet::all()).unwrap();
        let transcript = detail.transcript.unwrap();
        assert!(transcript.contains("Me: Hello there."));
        assert!(transcript.contains("Them: General Kenobi."));
        // Same speaker but a big pause still starts a fresh paragraph.
        assert_eq!(transcript.matches("Them:").count(), 2);
    }

    #[test]
    fn get_meeting_keeps_each_speaker_flowing_during_word_level_crosstalk() {
        let (conn, _dir, path) = fixture_db();
        insert_meeting(
            &conn,
            "m1",
            "Standup",
            "2026-01-01T10:00:00+00:00",
            &[
                ("hello", 0.0, 0.3, Some("me")),
                ("hi", 0.15, 0.35, Some("them")),
                ("how", 0.5, 0.7, Some("me")),
                ("good", 0.6, 0.8, Some("them")),
                ("are", 0.9, 1.1, Some("me")),
                ("thanks", 1.0, 1.2, Some("them")),
                ("you", 1.3, 1.5, Some("me")),
            ],
            None,
        );
        drop(conn);

        let db = McpDb::open(&path).unwrap();
        let transcript = db.get_meeting("m1", IncludeSet::all()).unwrap().transcript.unwrap();
        assert_eq!(
            transcript,
            "Me: hello how are you\n\nThem: hi good thanks"
        );
    }

    #[test]
    fn get_meeting_closes_an_interrupted_monologue_at_the_following_sentence_end() {
        // Me monologues without a pause; Them interjects mid-monologue. Me's
        // turn must not absorb everything: it closes at the first sentence
        // end after the interjection, so Them's interjection lands between
        // Me's two turns instead of trailing behind the whole monologue.
        let (conn, _dir, path) = fixture_db();
        insert_meeting(
            &conn,
            "m1",
            "Standup",
            "2026-01-01T10:00:00+00:00",
            &[
                ("Let me explain the whole plan", 0.0, 0.5, Some("me")),
                ("in detail because it's", 0.6, 1.1, Some("me")),
                ("wait", 1.2, 1.7, Some("them")),
                ("complicated.", 1.8, 2.3, Some("me")),
                ("So let's start now", 3.0, 3.5, Some("me")),
            ],
            None,
        );
        drop(conn);

        let db = McpDb::open(&path).unwrap();
        let transcript = db.get_meeting("m1", IncludeSet::all()).unwrap().transcript.unwrap();
        assert_eq!(
            transcript,
            "Me: Let me explain the whole plan in detail because it's complicated.\n\nThem: wait\n\nMe: So let's start now"
        );
    }

    #[test]
    fn get_meeting_ignores_a_sentence_end_before_the_interruption() {
        // An earlier sentence end, spoken before Them interjects, must not
        // retroactively split the turn: only the sentence end that comes
        // after the interjection closes it.
        let (conn, _dir, path) = fixture_db();
        insert_meeting(
            &conn,
            "m1",
            "Standup",
            "2026-01-01T10:00:00+00:00",
            &[
                ("First point.", 0.0, 0.5, Some("me")),
                ("Second part continues", 0.6, 1.1, Some("me")),
                ("quick question", 1.2, 1.7, Some("them")),
                ("and concludes.", 1.8, 2.3, Some("me")),
                ("New topic starts", 3.0, 3.5, Some("me")),
            ],
            None,
        );
        drop(conn);

        let db = McpDb::open(&path).unwrap();
        let transcript = db.get_meeting("m1", IncludeSet::all()).unwrap().transcript.unwrap();
        assert_eq!(
            transcript,
            "Me: First point. Second part continues and concludes.\n\nThem: quick question\n\nMe: New topic starts"
        );
    }

    #[test]
    fn get_meeting_prefers_edited_transcript() {
        let (conn, _dir, path) = fixture_db();
        conn.execute(
            "INSERT INTO meetings (id, title, started_at, duration_seconds, transcription_profile, recording_sessions, edited_transcript)
             VALUES ('m1', 'Standup', '2026-01-01T10:00:00+00:00', 10.0, '{}', '[]', 'hand-edited text')",
            [],
        )
        .unwrap();
        drop(conn);

        let db = McpDb::open(&path).unwrap();
        let detail = db.get_meeting("m1", IncludeSet::all()).unwrap();
        assert_eq!(detail.transcript.as_deref(), Some("hand-edited text"));
    }

    #[test]
    fn get_meeting_respects_include_filter() {
        let (conn, _dir, path) = fixture_db();
        conn.execute(
            "INSERT INTO meetings (id, title, started_at, duration_seconds, transcription_profile, recording_sessions, summary, notes)
             VALUES ('m1', 'Standup', '2026-01-01T10:00:00+00:00', 10.0, '{}', '[]', 'a summary', 'some notes')",
            [],
        )
        .unwrap();
        drop(conn);

        let db = McpDb::open(&path).unwrap();
        let names = vec!["summary".to_string()];
        let detail = db.get_meeting("m1", IncludeSet::from_names(Some(&names))).unwrap();
        assert_eq!(detail.summary.as_deref(), Some("a summary"));
        assert!(detail.transcript.is_none());
        assert!(detail.notes.is_none());
        assert!(detail.metadata.is_none());
    }

    #[test]
    fn get_meeting_missing_id_is_a_clean_error() {
        let (conn, _dir, path) = fixture_db();
        drop(conn);
        let db = McpDb::open(&path).unwrap();
        let err = db.get_meeting("nope", IncludeSet::all()).unwrap_err();
        assert!(matches!(err, McpDbError::MeetingNotFound(id) if id == "nope"));
    }

    #[test]
    fn get_meeting_resolves_persistent_speaker_names() {
        let (conn, _dir, path) = fixture_db();
        insert_speaker(&conn, 1, "Alice");
        insert_speaker(&conn, 2, "Bob");
        insert_meeting(
            &conn,
            "m1",
            "Standup",
            "2026-01-01T10:00:00+00:00",
            &[
                ("Hi Bob.", 0.0, 1.0, Some("spk:1")),
                ("Hi Alice.", 1.2, 2.0, Some("spk:2")),
            ],
            None,
        );
        drop(conn);

        let db = McpDb::open(&path).unwrap();
        let detail = db.get_meeting("m1", IncludeSet::all()).unwrap();
        let transcript = detail.transcript.unwrap();
        assert!(transcript.contains("Alice: Hi Bob."));
        assert!(transcript.contains("Bob: Hi Alice."));
        assert_eq!(
            detail.speakers.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(detail.speakers[0].name, "Alice");
        assert_eq!(detail.speakers[1].name, "Bob");
    }

    #[test]
    fn get_meeting_falls_back_to_speaker_id_when_speaker_row_is_missing() {
        let (conn, _dir, path) = fixture_db();
        insert_meeting(
            &conn,
            "m1",
            "Standup",
            "2026-01-01T10:00:00+00:00",
            &[("Orphaned reference.", 0.0, 1.0, Some("spk:99"))],
            None,
        );
        drop(conn);

        let db = McpDb::open(&path).unwrap();
        let detail = db.get_meeting("m1", IncludeSet::all()).unwrap();
        let transcript = detail.transcript.unwrap();
        assert!(transcript.contains("Speaker 99: Orphaned reference."));
        assert!(detail.speakers.is_empty());
    }

    #[test]
    fn get_meeting_speakers_empty_when_transcript_and_metadata_excluded() {
        let (conn, _dir, path) = fixture_db();
        insert_speaker(&conn, 1, "Alice");
        insert_meeting(
            &conn,
            "m1",
            "Standup",
            "2026-01-01T10:00:00+00:00",
            &[("Hello.", 0.0, 1.0, Some("spk:1"))],
            None,
        );
        drop(conn);

        let db = McpDb::open(&path).unwrap();
        let names = vec!["summary".to_string()];
        let detail = db
            .get_meeting("m1", IncludeSet::from_names(Some(&names)))
            .unwrap();
        assert!(detail.speakers.is_empty());
    }

    #[test]
    fn latest_meeting_picks_most_recent() {
        let (conn, _dir, path) = fixture_db();
        insert_meeting(&conn, "m1", "First", "2026-01-01T10:00:00+00:00", &[("hi", 0.0, 1.0, None)], None);
        insert_meeting(&conn, "m2", "Second", "2026-01-05T10:00:00+00:00", &[("hi", 0.0, 1.0, None)], None);
        drop(conn);

        let db = McpDb::open(&path).unwrap();
        let latest = db.latest_meeting(IncludeSet::all()).unwrap();
        assert_eq!(latest.id, "m2");
    }

    #[test]
    fn latest_meeting_empty_db_errors() {
        let (conn, _dir, path) = fixture_db();
        drop(conn);
        let db = McpDb::open(&path).unwrap();
        assert!(matches!(db.latest_meeting(IncludeSet::all()), Err(McpDbError::NoMeetings)));
    }

    #[test]
    fn search_meetings_returns_snippets() {
        let (conn, _dir, path) = fixture_db();
        insert_meeting(&conn, "m1", "Standup", "2026-01-01T10:00:00+00:00", &[("we discussed the roadmap today", 0.0, 1.0, None)], None);
        drop(conn);

        let db = McpDb::open(&path).unwrap();
        let hits = db.search_meetings("roadmap", 20).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "m1");
        assert!(hits[0].snippet.contains("**roadmap**"));
    }

    #[test]
    fn search_meetings_empty_query_returns_empty() {
        let (conn, _dir, path) = fixture_db();
        drop(conn);
        let db = McpDb::open(&path).unwrap();
        assert!(db.search_meetings("", 20).unwrap().is_empty());
    }

    #[test]
    fn list_dictations_orders_newest_first() {
        let (conn, _dir, path) = fixture_db();
        insert_dictation(&conn, "d1", "first note", "2026-01-01T10:00:00+00:00");
        insert_dictation(&conn, "d2", "second note", "2026-01-02T10:00:00+00:00");
        drop(conn);

        let db = McpDb::open(&path).unwrap();
        let entries = db.list_dictations(20).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "d2");
    }

    #[test]
    fn list_dictations_respects_limit() {
        let (conn, _dir, path) = fixture_db();
        for i in 0..5 {
            insert_dictation(&conn, &format!("d{i}"), "note", &format!("2026-01-0{}T10:00:00+00:00", i + 1));
        }
        drop(conn);

        let db = McpDb::open(&path).unwrap();
        assert_eq!(db.list_dictations(3).unwrap().len(), 3);
    }
}
