use chrono::{DateTime, Utc};
use rusqlite::params;

use crate::engine::TranscriptionSegment;
use crate::transcript::{MeetingListItem, MeetingTranscript};

use super::Database;

impl Database {
    /// Save a meeting with all its segments in a single transaction.
    /// Also indexes the full text for FTS5 search.
    pub fn save_meeting(&self, meeting: &MeetingTranscript) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| format!("Lock: {e}"))?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction: {e}"))?;

        tx.execute(
            "INSERT OR REPLACE INTO meetings (id, title, started_at, ended_at, duration_seconds, engine, summary, summary_model, summary_generated_at, edited_transcript)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                meeting.id,
                meeting.title,
                meeting.started_at.to_rfc3339(),
                meeting.ended_at.map(|dt| dt.to_rfc3339()),
                meeting.duration_seconds,
                meeting.engine,
                meeting.summary,
                meeting.summary_model,
                meeting.summary_generated_at.map(|dt| dt.to_rfc3339()),
                None::<String>, // edited_transcript
            ],
        )
        .map_err(|e| format!("Insert meeting: {e}"))?;

        // Delete existing segments for this meeting (for upsert scenarios)
        tx.execute(
            "DELETE FROM segments WHERE meeting_id = ?1",
            params![meeting.id],
        )
        .map_err(|e| format!("Delete segments: {e}"))?;

        for (i, seg) in meeting.segments.iter().enumerate() {
            tx.execute(
                "INSERT INTO segments (meeting_id, text, start_time, end_time, is_final, language, confidence, sort_order)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    meeting.id,
                    seg.text,
                    seg.start_time,
                    seg.end_time,
                    seg.is_final as i32,
                    seg.language,
                    seg.confidence,
                    i as i64,
                ],
            )
            .map_err(|e| format!("Insert segment: {e}"))?;
        }

        // Index full text for FTS5 search
        let full_text: String = meeting
            .segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        if !full_text.is_empty() {
            // Remove old FTS entry
            tx.execute(
                "DELETE FROM text_search WHERE source_type = 'meeting' AND source_id = ?1",
                params![meeting.id],
            )
            .map_err(|e| format!("Delete FTS: {e}"))?;

            tx.execute(
                "INSERT INTO text_search (content, source_type, source_id) VALUES (?1, ?2, ?3)",
                params![full_text, "meeting", meeting.id],
            )
            .map_err(|e| format!("Insert FTS: {e}"))?;
        }

        tx.commit().map_err(|e| format!("Commit: {e}"))?;
        Ok(())
    }

    /// Load a full meeting with segments by ID.
    pub fn load_meeting(&self, id: &str) -> Result<MeetingTranscript, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock: {e}"))?;

        let meeting = conn
            .query_row(
                "SELECT id, title, started_at, ended_at, duration_seconds, engine, summary, summary_model, summary_generated_at
                 FROM meetings WHERE id = ?1",
                params![id],
                |row| {
                    Ok(MeetingRow {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        started_at: row.get::<_, String>(2)?,
                        ended_at: row.get::<_, Option<String>>(3)?,
                        duration_seconds: row.get(4)?,
                        engine: row.get(5)?,
                        summary: row.get(6)?,
                        summary_model: row.get(7)?,
                        summary_generated_at: row.get::<_, Option<String>>(8)?,
                    })
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => format!("Meeting not found: {id}"),
                _ => format!("Query: {e}"),
            })?;

        let mut stmt = conn
            .prepare(
                "SELECT text, start_time, end_time, is_final, language, confidence
                 FROM segments WHERE meeting_id = ?1 ORDER BY sort_order",
            )
            .map_err(|e| format!("Prepare: {e}"))?;

        let segments = stmt
            .query_map(params![id], |row| {
                Ok(TranscriptionSegment {
                    text: row.get(0)?,
                    start_time: row.get(1)?,
                    end_time: row.get(2)?,
                    is_final: row.get::<_, i32>(3)? != 0,
                    language: row.get(4)?,
                    confidence: row.get(5)?,
                })
            })
            .map_err(|e| format!("Query segments: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Collect segments: {e}"))?;

        Ok(MeetingTranscript {
            id: meeting.id,
            title: meeting.title,
            started_at: parse_datetime(&meeting.started_at)?,
            ended_at: meeting
                .ended_at
                .as_deref()
                .map(parse_datetime)
                .transpose()?,
            duration_seconds: meeting.duration_seconds,
            engine: meeting.engine,
            segments,
            summary: meeting.summary,
            summary_model: meeting.summary_model,
            summary_generated_at: meeting
                .summary_generated_at
                .as_deref()
                .map(parse_datetime)
                .transpose()?,
        })
    }

    /// List all meetings (lightweight, no segments).
    pub fn list_meetings(&self) -> Result<Vec<MeetingListItem>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock: {e}"))?;

        let mut stmt = conn
            .prepare(
                "SELECT id, title, started_at, duration_seconds, summary IS NOT NULL
                 FROM meetings ORDER BY started_at DESC",
            )
            .map_err(|e| format!("Prepare: {e}"))?;

        let items = stmt
            .query_map([], |row| {
                Ok(MeetingListItem {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    started_at: row.get::<_, String>(2).and_then(|s| {
                        DateTime::parse_from_rfc3339(&s)
                            .map(|dt| dt.with_timezone(&Utc))
                            .map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    2,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })
                    })?,
                    duration_seconds: row.get(3)?,
                    has_summary: row.get(4)?,
                })
            })
            .map_err(|e| format!("Query: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Collect: {e}"))?;

        Ok(items)
    }

    /// Delete a meeting and its segments (CASCADE handles segments).
    pub fn delete_meeting(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock: {e}"))?;

        // Delete FTS entry
        conn.execute(
            "DELETE FROM text_search WHERE source_type = 'meeting' AND source_id = ?1",
            params![id],
        )
        .map_err(|e| format!("Delete FTS: {e}"))?;

        // Delete embeddings
        conn.execute("DELETE FROM embeddings WHERE meeting_id = ?1", params![id])
            .map_err(|e| format!("Delete embeddings: {e}"))?;

        // Delete meeting (CASCADE deletes segments)
        conn.execute("DELETE FROM meetings WHERE id = ?1", params![id])
            .map_err(|e| format!("Delete meeting: {e}"))?;

        Ok(())
    }

    /// Update meeting summary fields.
    pub fn update_meeting_summary(
        &self,
        id: &str,
        summary: &str,
        model: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock: {e}"))?;
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "UPDATE meetings SET summary = ?1, summary_model = ?2, summary_generated_at = ?3 WHERE id = ?4",
            params![summary, model, now, id],
        )
        .map_err(|e| format!("Update summary: {e}"))?;

        Ok(())
    }

    /// Check if a meeting with the given ID exists.
    pub fn meeting_exists(&self, id: &str) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock: {e}"))?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM meetings WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Query: {e}"))?;
        Ok(count > 0)
    }
}

/// Intermediate struct for reading meeting rows
struct MeetingRow {
    id: String,
    title: String,
    started_at: String,
    ended_at: Option<String>,
    duration_seconds: f64,
    engine: String,
    summary: Option<String>,
    summary_model: Option<String>,
    summary_generated_at: Option<String>,
}

fn parse_datetime(s: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| format!("Parse datetime '{s}': {e}"))
}
