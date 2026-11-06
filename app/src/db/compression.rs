//! Short, coherent source snapshots and compare-and-update compact persistence.
use super::Database;
use crate::{
    engine::{Speaker, TranscriptionSegment},
    lock_ext::MutexExt,
    summary::compress::{CompressionError, CompressionInput},
    transcript::{MeetingParticipant, MeetingRecordingSession},
};
use rusqlite::{Connection, OptionalExtension, params};
use souffle_schema::{
    CompactTranscript,
    sql::{
        column::*,
        table::{MEETINGS, SEGMENTS},
    },
};
#[derive(Debug)]
pub struct CompactionSnapshot {
    pub input: CompressionInput,
    pub token: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactCommit {
    Committed,
    Superseded,
    Deleted,
}

pub(crate) fn source(
    conn: &Connection,
    id: &str,
) -> Result<Option<(CompressionInput, bool)>, String> {
    let header=conn.query_row(&format!("SELECT {EDITED_TRANSCRIPT},{PARTICIPANTS},{RECORDING_SESSIONS},{ENDED_AT} FROM {MEETINGS} WHERE {ID}=?1"),[id],|row|Ok((row.get::<_,Option<String>>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,String>(2)?,row.get::<_,Option<String>>(3)?))).optional().map_err(|e|e.to_string())?;
    let Some((edited, participants, sessions, ended)) = header else {
        return Ok(None);
    };
    let participants: Vec<MeetingParticipant> = participants
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let sessions: Vec<MeetingRecordingSession> =
        serde_json::from_str(&sessions).map_err(|e| e.to_string())?;
    let edited = edited.filter(|s| !s.is_empty());
    let segmented = edited.is_none();
    let lines = match edited {
        Some(text) => text.split('\n').map(str::to_owned).collect(),
        None => {
            let mut stmt=conn.prepare(&format!("SELECT {TEXT},{START_TIME},{END_TIME},{IS_FINAL},{LANGUAGE},{CONFIDENCE},{SPEAKER} FROM {SEGMENTS} WHERE {MEETING_ID}=?1 ORDER BY {SORT_ORDER}")).map_err(|e|e.to_string())?;
            let segments = stmt
                .query_map([id], |r| {
                    Ok(TranscriptionSegment {
                        text: r.get(0)?,
                        start_time: r.get(1)?,
                        end_time: r.get(2)?,
                        is_final: r.get(3)?,
                        language: r.get(4)?,
                        confidence: r.get(5)?,
                        speaker: r
                            .get::<_, Option<String>>(6)?
                            .as_deref()
                            .and_then(Speaker::parse),
                    })
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            crate::summary::turns_from_segments(&segments)
        }
    };
    Ok(Some((
        CompressionInput::new(
            lines,
            participants.into_iter().map(|p| p.name).collect(),
            &sessions,
            ended.is_none(),
            segmented,
        ),
        ended.is_none(),
    )))
}
pub(crate) fn fingerprint(conn: &Connection, id: &str) -> Result<Option<String>, String> {
    Ok(source(conn, id)?.map(|(input, _)| input.source_sha256))
}
/// A mutation pays for source fingerprints only when a compact or an active
/// generation can be invalidated. Skipped does not mean the row is absent.
#[derive(Debug, PartialEq)]
pub(crate) enum InvalidationSnapshot {
    Untracked,
    Tracked(Option<String>),
}
pub(crate) fn invalidation_snapshot(
    conn: &Connection,
    id: &str,
) -> Result<InvalidationSnapshot, String> {
    let tracked=conn.query_row(&format!("SELECT {COMPACT_TRANSCRIPT} IS NOT NULL OR {COMPACT_GENERATION_TOKEN} IS NOT NULL FROM {MEETINGS} WHERE {ID}=?1"),[id],|row|row.get::<_,bool>(0)).optional().map_err(|e|e.to_string())?.unwrap_or(false);
    if tracked {
        Ok(InvalidationSnapshot::Tracked(fingerprint(conn, id)?))
    } else {
        Ok(InvalidationSnapshot::Untracked)
    }
}
pub(crate) fn invalidate_changed(
    conn: &Connection,
    id: &str,
    before: InvalidationSnapshot,
) -> Result<(), String> {
    match before {
        InvalidationSnapshot::Untracked => {}
        InvalidationSnapshot::Tracked(before) => {
            if before != fingerprint(conn, id)? {
                conn.execute(&format!("UPDATE {MEETINGS} SET {COMPACT_GENERATION_TOKEN}=NULL,{COMPACT_IS_STALE}=CASE WHEN {COMPACT_TRANSCRIPT} IS NOT NULL THEN 1 ELSE {COMPACT_IS_STALE} END WHERE {ID}=?1"),[id]).map_err(|e|e.to_string())?;
            }
        }
    }
    Ok(())
}
pub(crate) fn read_compact(
    conn: &Connection,
    id: &str,
) -> Result<Option<CompactTranscript>, String> {
    conn.query_row(&format!("SELECT {COMPACT_TRANSCRIPT},{COMPACT_MODEL},{COMPACT_RATE},{COMPACT_GENERATED_AT},{COMPACT_IS_STALE},{COMPACT_SOURCE_SHA256} FROM {MEETINGS} WHERE {ID}=?1"),[id],|row|{
        let text:Option<String>=row.get(0)?;
        text.map(|text|Ok(CompactTranscript{approx_tokens:souffle_schema::estimate_tokens(&text),text,model:row.get(1)?,rate:row.get(2)?,generated_at:row.get(3)?,is_stale:row.get(4)?,source_sha256:row.get(5)?})).transpose()
    }).optional().map(|o|o.flatten()).map_err(|e|e.to_string())
}
pub(crate) fn write_compact(
    conn: &Connection,
    id: &str,
    c: &CompactTranscript,
) -> Result<(), String> {
    conn.execute(&format!("UPDATE {MEETINGS} SET {COMPACT_TRANSCRIPT}=?2,{COMPACT_MODEL}=?3,{COMPACT_RATE}=?4,{COMPACT_GENERATED_AT}=?5,{COMPACT_IS_STALE}=?6,{COMPACT_SOURCE_SHA256}=?7 WHERE {ID}=?1"),params![id,c.text,c.model,c.rate,c.generated_at,c.is_stale,c.source_sha256]).map_err(|e|e.to_string())?;
    Ok(())
}
impl Database {
    /// Reserve a fresh generation and effective source in one short transaction,
    /// refusing deleted, ongoing or empty meetings before inference.
    pub fn begin_compaction(&self, id: &str) -> Result<CompactionSnapshot, CompressionError> {
        let mut conn = self
            .conn
            .acquire()
            .map_err(|detail| CompressionError::Database { detail })?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| CompressionError::Database {
                detail: e.to_string(),
            })?;
        let (input, ongoing) = source(&tx, id)
            .map_err(|detail| CompressionError::Database { detail })?
            .ok_or(CompressionError::Deleted)?;
        if ongoing {
            return Err(CompressionError::Ongoing);
        }
        if input.lines.iter().all(|s| s.trim().is_empty()) {
            return Err(CompressionError::EmptyTranscript);
        }
        let token = uuid::Uuid::new_v4().to_string();
        tx.execute(
            &format!("UPDATE {MEETINGS} SET {COMPACT_GENERATION_TOKEN}=?2 WHERE {ID}=?1"),
            params![id, token],
        )
        .map_err(|e| CompressionError::Database {
            detail: e.to_string(),
        })?;
        tx.commit().map_err(|e| CompressionError::Database {
            detail: e.to_string(),
        })?;
        Ok(CompactionSnapshot { input, token })
    }
    /// Publish only when the finalized source and generation still match, using
    /// UPDATE rather than insertion so a completed job cannot resurrect a meeting.
    pub fn commit_compaction(
        &self,
        id: &str,
        snapshot: &CompactionSnapshot,
        c: &CompactTranscript,
    ) -> Result<CompactCommit, String> {
        let mut conn = self.conn.acquire()?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let Some((current, ongoing)) = source(&tx, id)? else {
            return Ok(CompactCommit::Deleted);
        };
        let token: Option<String> = tx
            .query_row(
                &format!("SELECT {COMPACT_GENERATION_TOKEN} FROM {MEETINGS} WHERE {ID}=?1"),
                [id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if ongoing
            || current.source_sha256 != snapshot.input.source_sha256
            || c.source_sha256 != current.source_sha256
            || token.as_deref() != Some(&snapshot.token)
        {
            return Ok(CompactCommit::Superseded);
        }
        let affected=tx.execute(&format!("UPDATE {MEETINGS} SET {COMPACT_TRANSCRIPT}=?2,{COMPACT_MODEL}=?3,{COMPACT_RATE}=?4,{COMPACT_GENERATED_AT}=?5,{COMPACT_IS_STALE}=?6,{COMPACT_SOURCE_SHA256}=?7 WHERE {ID}=?1 AND {COMPACT_GENERATION_TOKEN}=?8"),params![id,c.text,c.model,c.rate,c.generated_at,c.is_stale,c.source_sha256,snapshot.token]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(if affected == 1 {
            CompactCommit::Committed
        } else {
            CompactCommit::Superseded
        })
    }
    /// Read the single optional compact object without loading meeting segments.
    pub fn load_compact_transcript(&self, id: &str) -> Result<Option<CompactTranscript>, String> {
        {
            let conn = self.conn.acquire()?;
            read_compact(&conn, id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::fixtures::{sample_meeting, sample_segment, test_db};
    fn compact(snapshot: &CompactionSnapshot) -> CompactTranscript {
        CompactTranscript {
            text: "[0:00] Hello 42".into(),
            model: "synthetic-policy".into(),
            rate: 0.5,
            generated_at: chrono::Utc::now().to_rfc3339(),
            is_stale: false,
            source_sha256: snapshot.input.source_sha256.clone(),
            approx_tokens: 6,
        }
    }
    fn seed(db: &Database, id: &str) {
        db.save_meeting(&sample_meeting(id)).unwrap();
        let s = db.begin_compaction(id).unwrap();
        assert_eq!(
            db.commit_compaction(id, &s, &compact(&s)).unwrap(),
            CompactCommit::Committed
        );
    }
    #[test]
    fn default_off_mutations_skip_hashes_but_active_generation_is_invalidated() {
        let (db, _dir) = test_db();
        db.save_meeting(&sample_meeting("default-off")).unwrap();
        // Malformed source metadata proves both before/after hashes are skipped.
        {
            let conn = db.conn.acquire().unwrap();
            conn.execute(
                &format!("UPDATE {MEETINGS} SET {RECORDING_SESSIONS}='invalid' WHERE {ID}=?1"),
                ["default-off"],
            )
            .unwrap();
        }
        db.update_segment_texts("default-off", &[(0, "changed".into())])
            .unwrap();
        db.append_segments("default-off", &[sample_segment("new", 4., 5.)], 2)
            .unwrap();
        db.save_edited_transcript("default-off", Some("edited"))
            .unwrap();
        {
            let conn = db.conn.acquire().unwrap();
            assert_eq!(
                invalidation_snapshot(&conn, "default-off").unwrap(),
                InvalidationSnapshot::Untracked
            );
        }
        db.save_meeting(&sample_meeting("active-generation"))
            .unwrap();
        let active = db.begin_compaction("active-generation").unwrap();
        assert!(
            db.load_compact_transcript("active-generation")
                .unwrap()
                .is_none()
        );
        db.save_edited_transcript("active-generation", Some("source changed"))
            .unwrap();
        assert_eq!(
            db.commit_compaction("active-generation", &active, &compact(&active))
                .unwrap(),
            CompactCommit::Superseded
        );
        let conn = db.conn.acquire().unwrap();
        let token: Option<String> = conn
            .query_row(
                &format!("SELECT {COMPACT_GENERATION_TOKEN} FROM {MEETINGS} WHERE {ID}=?1"),
                ["active-generation"],
                |row| row.get(0),
            )
            .unwrap();
        assert!(token.is_none());
    }
    #[test]
    fn compact_source_cas_and_preserving_full_save() {
        let (db, _dir) = test_db();
        seed(&db, "m");
        let original = db.load_meeting("m").unwrap();
        let older = db.begin_compaction("m").unwrap();
        let newer = db.begin_compaction("m").unwrap();
        assert_eq!(
            db.commit_compaction("m", &older, &compact(&older)).unwrap(),
            CompactCommit::Superseded
        );
        assert_eq!(
            db.commit_compaction("m", &newer, &compact(&newer)).unwrap(),
            CompactCommit::Committed
        );
        let mut old_snapshot = original.clone();
        old_snapshot.compact_transcript = None;
        old_snapshot.notes = Some("Notes alone".into());
        db.save_meeting(&old_snapshot).unwrap();
        assert!(!db.load_compact_transcript("m").unwrap().unwrap().is_stale);
        db.save_edited_transcript("m", None).unwrap();
        assert!(!db.load_compact_transcript("m").unwrap().unwrap().is_stale);
        let stale_job = db.begin_compaction("m").unwrap();
        db.save_edited_transcript("m", Some("Plain edited Élodie ne paie pas 42 euros"))
            .unwrap();
        assert!(db.load_compact_transcript("m").unwrap().unwrap().is_stale);
        assert_eq!(
            db.commit_compaction("m", &stale_job, &compact(&stale_job))
                .unwrap(),
            CompactCommit::Superseded
        );
        let edited = db.begin_compaction("m").unwrap();
        assert_eq!(
            edited.input.lines,
            ["Plain edited Élodie ne paie pas 42 euros"]
        );
        assert_eq!(
            db.commit_compaction("m", &edited, &compact(&edited))
                .unwrap(),
            CompactCommit::Committed
        );
        db.save_edited_transcript("m", Some("Plain edited Élodie ne paie pas 42 euros"))
            .unwrap();
        assert!(!db.load_compact_transcript("m").unwrap().unwrap().is_stale);
        let gone = db.begin_compaction("m").unwrap();
        db.delete_meeting("m").unwrap();
        assert_eq!(
            db.commit_compaction("m", &gone, &compact(&gone)).unwrap(),
            CompactCommit::Deleted
        );
        assert!(!db.meeting_exists("m").unwrap());
    }
    #[test]
    fn compact_segment_append_resume_and_full_source_changes() {
        let (db, _dir) = test_db();
        seed(&db, "segments");
        db.update_segment_texts("segments", &[(0, "Hello world".into())])
            .unwrap();
        assert!(
            !db.load_compact_transcript("segments")
                .unwrap()
                .unwrap()
                .is_stale
        );
        db.update_segment_texts("segments", &[(0, "changed".into())])
            .unwrap();
        assert!(
            db.load_compact_transcript("segments")
                .unwrap()
                .unwrap()
                .is_stale
        );
        seed(&db, "append");
        db.append_segments("append", &[sample_segment("Another paragraph", 4., 5.)], 2)
            .unwrap();
        assert!(
            db.load_compact_transcript("append")
                .unwrap()
                .unwrap()
                .is_stale
        );
        seed(&db, "resume");
        db.upsert_meeting_header(&sample_meeting("resume")).unwrap();
        assert!(
            db.load_compact_transcript("resume")
                .unwrap()
                .unwrap()
                .is_stale
        );
        assert!(db.begin_compaction("resume").is_err());
        seed(&db, "save");
        let mut changed = db.load_meeting("save").unwrap();
        changed
            .participants
            .push(crate::transcript::MeetingParticipant {
                name: "Élodie".into(),
                email: None,
                is_organizer: false,
                is_current_user: false,
            });
        db.save_meeting(&changed).unwrap();
        assert!(
            db.load_compact_transcript("save")
                .unwrap()
                .unwrap()
                .is_stale
        );
    }
}
