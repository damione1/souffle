use chrono::{DateTime, Utc};
use rusqlite::params;

use crate::engine::{TranscriptionProfile, TranscriptionSegment};
use crate::transcript::{
    MeetingListItem, MeetingTranscript, resolve_legacy_transcription_profile,
};

use crate::lock_ext::MutexExt;

use super::Database;

impl Database {
    /// Save a meeting with all its segments in a single transaction.
    /// Also indexes the full text for FTS5 search.
    pub fn save_meeting(&self, meeting: &MeetingTranscript) -> Result<(), String> {
        let mut conn = self.conn.acquire()?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction: {e}"))?;

        tx.execute(
            "INSERT OR REPLACE INTO meetings (id, title, started_at, ended_at, duration_seconds, engine, transcription_profile, summary, summary_model, summary_generated_at, edited_transcript)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                meeting.id,
                meeting.title,
                meeting.started_at.to_rfc3339(),
                meeting.ended_at.map(|dt| dt.to_rfc3339()),
                meeting.duration_seconds,
                meeting.transcription_profile.engine_label,
                serde_json::to_string(&meeting.transcription_profile)
                    .map_err(|e| format!("Serialize profile: {e}"))?,
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
        let conn = self.conn.acquire()?;

        let meeting = conn
            .query_row(
                "SELECT id, title, started_at, ended_at, duration_seconds, engine, transcription_profile, summary, summary_model, summary_generated_at
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
                        transcription_profile: row.get(6)?,
                        summary: row.get(7)?,
                        summary_model: row.get(8)?,
                        summary_generated_at: row.get::<_, Option<String>>(9)?,
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

        let transcription_profile = meeting.transcription_profile()?;

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
            transcription_profile,
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
        let conn = self.conn.acquire()?;

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
        let conn = self.conn.acquire()?;

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
        let conn = self.conn.acquire()?;
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
        let conn = self.conn.acquire()?;
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
    transcription_profile: Option<String>,
    summary: Option<String>,
    summary_model: Option<String>,
    summary_generated_at: Option<String>,
}

impl MeetingRow {
    fn transcription_profile(&self) -> Result<TranscriptionProfile, String> {
        let profile = self
            .transcription_profile
            .as_deref()
            .map(serde_json::from_str::<TranscriptionProfile>)
            .transpose()
            .map_err(|e| format!("Deserialize transcription profile: {e}"))?;

        resolve_legacy_transcription_profile(profile, Some(&self.engine))
    }
}

fn parse_datetime(s: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| format!("Parse datetime '{s}': {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::engine::TranscriptionSegment;
    use crate::transcript::MeetingTranscript;
    use tempfile::TempDir;

    fn test_db() -> (Database, TempDir) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        (db, dir)
    }

    fn sample_meeting(id: &str) -> MeetingTranscript {
        MeetingTranscript {
            id: id.to_string(),
            title: "Test Meeting".to_string(),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            duration_seconds: 60.0,
            transcription_profile: TranscriptionProfile::default(),
            segments: vec![
                TranscriptionSegment {
                    text: "Hello world".to_string(),
                    start_time: 0.0,
                    end_time: 1.0,
                    is_final: true,
                    language: Some("en".to_string()),
                    confidence: Some(0.95),
                },
                TranscriptionSegment {
                    text: "second segment".to_string(),
                    start_time: 1.5,
                    end_time: 2.5,
                    is_final: true,
                    language: None,
                    confidence: None,
                },
            ],
            summary: None,
            summary_model: None,
            summary_generated_at: None,
        }
    }

    #[test]
    fn save_and_load_meeting() {
        let (db, _dir) = test_db();
        let meeting = sample_meeting("m1");
        db.save_meeting(&meeting).unwrap();

        let loaded = db.load_meeting("m1").unwrap();
        assert_eq!(loaded.id, "m1");
        assert_eq!(loaded.title, "Test Meeting");
        assert_eq!(loaded.segments.len(), 2);
        assert_eq!(loaded.segments[0].text, "Hello world");
        assert_eq!(loaded.segments[1].text, "second segment");
        assert_eq!(
            loaded.transcription_profile,
            TranscriptionProfile::default()
        );
    }

    #[test]
    fn list_meetings() {
        let (db, _dir) = test_db();
        db.save_meeting(&sample_meeting("m1")).unwrap();
        db.save_meeting(&sample_meeting("m2")).unwrap();

        let list = db.list_meetings().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn delete_meeting_cascades() {
        let (db, _dir) = test_db();
        db.save_meeting(&sample_meeting("m1")).unwrap();
        assert!(db.meeting_exists("m1").unwrap());

        db.delete_meeting("m1").unwrap();
        assert!(!db.meeting_exists("m1").unwrap());
    }

    #[test]
    fn update_summary() {
        let (db, _dir) = test_db();
        db.save_meeting(&sample_meeting("m1")).unwrap();
        db.update_meeting_summary("m1", "Summary text", "qwen2.5")
            .unwrap();

        let loaded = db.load_meeting("m1").unwrap();
        assert_eq!(loaded.summary.as_deref(), Some("Summary text"));
        assert_eq!(loaded.summary_model.as_deref(), Some("qwen2.5"));
        assert!(loaded.summary_generated_at.is_some());
    }

    #[test]
    fn load_nonexistent_meeting_returns_error() {
        let (db, _dir) = test_db();
        assert!(db.load_meeting("nonexistent").is_err());
    }

    #[test]
    fn load_meeting_uses_legacy_engine_when_profile_missing() {
        let (db, _dir) = test_db();
        let conn = db.conn.acquire().unwrap();
        conn.execute(
            "INSERT INTO meetings (id, title, started_at, ended_at, duration_seconds, engine, summary, summary_model, summary_generated_at, edited_transcript)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                "legacy-id",
                "Legacy Meeting",
                Utc::now().to_rfc3339(),
                None::<String>,
                12.0,
                "Legacy Engine",
                None::<String>,
                None::<String>,
                None::<String>,
                None::<String>,
            ],
        )
        .unwrap();
        drop(conn);

        let loaded = db.load_meeting("legacy-id").unwrap();
        assert_eq!(loaded.transcription_profile.engine_label, "Legacy Engine");
        assert_eq!(loaded.transcription_profile.model_id, "legacy");
    }
}
