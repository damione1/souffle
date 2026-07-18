use chrono::{DateTime, Utc};
use rusqlite::params;

use crate::engine::{Speaker, TranscriptionProfile, TranscriptionSegment};
use crate::lock_ext::MutexExt;
use crate::transcript::{
    MeetingListItem, MeetingParticipant, MeetingRecordingSession, MeetingSpeaker,
    MeetingTranscript, StructuredSummary,
};

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
            "INSERT OR REPLACE INTO meetings (
                id,
                title,
                started_at,
                ended_at,
                duration_seconds,
                transcription_profile,
                recording_sessions,
                summary,
                summary_is_stale,
                summary_model,
                summary_generated_at,
                edited_transcript,
                notes,
                calendar_event_id,
                participants,
                structured_summary
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                meeting.id,
                meeting.title,
                meeting.started_at.to_rfc3339(),
                meeting.ended_at.map(|dt| dt.to_rfc3339()),
                meeting.duration_seconds,
                serde_json::to_string(&meeting.transcription_profile)
                    .map_err(|e| format!("Serialize profile: {e}"))?,
                serde_json::to_string(&meeting.recording_sessions)
                    .map_err(|e| format!("Serialize recording sessions: {e}"))?,
                meeting.summary,
                i32::from(meeting.summary_is_stale),
                meeting.summary_model,
                meeting.summary_generated_at.map(|dt| dt.to_rfc3339()),
                meeting.edited_transcript,
                meeting.notes,
                meeting.calendar_event_id,
                serialize_participants(&meeting.participants)?,
                serialize_structured_summary(meeting.structured_summary.as_ref())?,
            ],
        )
        .map_err(|e| format!("Insert meeting: {e}"))?;

        tx.execute(
            "DELETE FROM segments WHERE meeting_id = ?1",
            params![meeting.id],
        )
        .map_err(|e| format!("Delete segments: {e}"))?;

        for (i, seg) in meeting.segments.iter().enumerate() {
            tx.execute(
                "INSERT INTO segments (meeting_id, text, start_time, end_time, is_final, language, confidence, sort_order, speaker)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    meeting.id,
                    seg.text,
                    seg.start_time,
                    seg.end_time,
                    seg.is_final as i32,
                    seg.language,
                    seg.confidence,
                    i as i64,
                    seg.speaker.map(Speaker::as_str),
                ],
            )
            .map_err(|e| format!("Insert segment: {e}"))?;
        }

        tx.execute(
            "DELETE FROM text_search WHERE source_type = 'meeting' AND source_id = ?1",
            params![meeting.id],
        )
        .map_err(|e| format!("Delete FTS: {e}"))?;

        let full_text = meeting
            .segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        if !full_text.is_empty() {
            tx.execute(
                "INSERT INTO text_search (content, source_type, source_id) VALUES (?1, ?2, ?3)",
                params![full_text, "meeting", meeting.id],
            )
            .map_err(|e| format!("Insert FTS: {e}"))?;
        }

        tx.commit().map_err(|e| format!("Commit: {e}"))?;
        Ok(())
    }

    /// Write only the `meetings` header row (no segments, no FTS), creating it
    /// if absent or refreshing the header fields if it exists. Used at the start
    /// of a recording so segment rows have a valid FK target and a crash leaves
    /// a row with `ended_at IS NULL` for recovery to finalize.
    ///
    /// On resume (conflict) `ended_at` is reset to NULL and `edited_transcript`
    /// is deliberately left untouched so a user's edits survive re-recording.
    pub fn upsert_meeting_header(&self, meeting: &MeetingTranscript) -> Result<(), String> {
        let conn = self.conn.acquire()?;
        conn.execute(
            "INSERT INTO meetings (
                id, title, started_at, ended_at, duration_seconds,
                transcription_profile, recording_sessions, summary,
                summary_is_stale, summary_model, summary_generated_at,
                edited_transcript, notes, calendar_event_id, participants,
                structured_summary
             ) VALUES (?1, ?2, ?3, NULL, 0, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, ?11, ?12, ?13)
             ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                started_at = excluded.started_at,
                ended_at = NULL,
                transcription_profile = excluded.transcription_profile,
                recording_sessions = excluded.recording_sessions,
                summary = excluded.summary,
                summary_is_stale = excluded.summary_is_stale,
                summary_model = excluded.summary_model,
                summary_generated_at = excluded.summary_generated_at,
                notes = excluded.notes,
                calendar_event_id = excluded.calendar_event_id,
                participants = excluded.participants,
                structured_summary = excluded.structured_summary",
            params![
                meeting.id,
                meeting.title,
                meeting.started_at.to_rfc3339(),
                serde_json::to_string(&meeting.transcription_profile)
                    .map_err(|e| format!("Serialize profile: {e}"))?,
                serde_json::to_string(&meeting.recording_sessions)
                    .map_err(|e| format!("Serialize recording sessions: {e}"))?,
                meeting.summary,
                i32::from(meeting.summary_is_stale),
                meeting.summary_model,
                meeting.summary_generated_at.map(|dt| dt.to_rfc3339()),
                meeting.notes,
                meeting.calendar_event_id,
                serialize_participants(&meeting.participants)?,
                serialize_structured_summary(meeting.structured_summary.as_ref())?,
            ],
        )
        .map_err(|e| format!("Upsert meeting header: {e}"))?;
        Ok(())
    }

    /// Append a batch of segments to an existing meeting in one transaction,
    /// numbering them `start_sort_order..`. Append-only (no DELETE) so it is safe
    /// to call repeatedly during a live meeting for crash durability.
    pub fn append_segments(
        &self,
        meeting_id: &str,
        segments: &[TranscriptionSegment],
        start_sort_order: i64,
    ) -> Result<(), String> {
        if segments.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.acquire()?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction: {e}"))?;
        for (i, seg) in segments.iter().enumerate() {
            tx.execute(
                "INSERT INTO segments (meeting_id, text, start_time, end_time, is_final, language, confidence, sort_order, speaker)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    meeting_id,
                    seg.text,
                    seg.start_time,
                    seg.end_time,
                    seg.is_final as i32,
                    seg.language,
                    seg.confidence,
                    start_sort_order + i as i64,
                    seg.speaker.map(Speaker::as_str),
                ],
            )
            .map_err(|e| format!("Append segment: {e}"))?;
        }
        tx.commit().map_err(|e| format!("Commit: {e}"))?;
        Ok(())
    }

    /// Patch segment text for segments already flushed during a live meeting.
    /// `updates` pairs `(sort_order, new_text)`.
    pub fn update_segment_texts(
        &self,
        meeting_id: &str,
        updates: &[(i64, String)],
    ) -> Result<(), String> {
        if updates.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.acquire()?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction: {e}"))?;
        for (sort_order, text) in updates {
            tx.execute(
                "UPDATE segments SET text = ?3 WHERE meeting_id = ?1 AND sort_order = ?2",
                params![meeting_id, sort_order, text],
            )
            .map_err(|e| format!("Update segment text: {e}"))?;
        }
        tx.commit().map_err(|e| format!("Commit: {e}"))?;
        Ok(())
    }

    /// Set the speaker label on specific segments of a meeting, identified
    /// by their `sort_order`, in one transaction. Segments whose speaker is
    /// currently NULL are always writable. `replaceable` optionally names the
    /// one live-attribution label (Me or Them) this write may also overwrite,
    /// so an offline diarization pass can refine its own lane: the mic pass
    /// replaces `me`, the system pass replaces `them`. Any other existing
    /// label (the opposite lane, previous diarization passes) is never
    /// touched. Segment text is untouched, so the FTS index and any edited
    /// transcript stay valid as-is. Returns how many rows actually changed.
    pub fn set_segment_speakers(
        &self,
        meeting_id: &str,
        assignments: &[(u64, Speaker)],
        replaceable: Option<Speaker>,
    ) -> Result<usize, String> {
        if assignments.is_empty() {
            return Ok(0);
        }
        if matches!(replaceable, Some(Speaker::Persistent(_))) {
            return Err(
                "Persistent labels cannot be marked replaceable; use retag_persistent_speaker"
                    .into(),
            );
        }
        let replaceable_label = replaceable.map(Speaker::as_str);
        let mut conn = self.conn.acquire()?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction: {e}"))?;
        let mut changed = 0usize;
        for (sort_order, speaker) in assignments {
            // `speaker = NULL` is never true in SQL, so a `replaceable` of
            // `None` reduces the predicate to `speaker IS NULL`.
            changed += tx
                .execute(
                    "UPDATE segments SET speaker = ?1
                     WHERE meeting_id = ?2 AND sort_order = ?3
                       AND (speaker IS NULL OR speaker = ?4)",
                    params![
                        speaker.as_str(),
                        meeting_id,
                        *sort_order as i64,
                        replaceable_label
                    ],
                )
                .map_err(|e| format!("Set segment speaker: {e}"))?;
        }
        tx.commit().map_err(|e| format!("Commit: {e}"))?;
        Ok(changed)
    }

    /// Reassign persistent-speaker labels within one meeting. Only segments
    /// currently labeled `spk:from_persistent_id` are rewritten; Me/Them and
    /// NULL speakers are left alone. When `sort_orders` is `Some`, only those
    /// segment indices are retagged; when `None`, every matching segment in
    /// the meeting is retagged. Unlike `set_segment_speakers`, this overwrites
    /// existing `spk:*` labels. The target must be `Speaker::Persistent`.
    /// Returns how many rows changed.
    pub fn retag_persistent_speaker(
        &self,
        meeting_id: &str,
        from_persistent_id: i64,
        to_speaker: Speaker,
        sort_orders: Option<&[u64]>,
    ) -> Result<usize, String> {
        let Speaker::Persistent(to_id) = to_speaker else {
            return Err("Retag target must be a persistent speaker".into());
        };
        let from_label = Speaker::Persistent(from_persistent_id).as_str();
        let to_label = Speaker::Persistent(to_id).as_str();
        let mut conn = self.conn.acquire()?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction: {e}"))?;
        let changed = execute_retag_persistent_speaker(
            &tx,
            meeting_id,
            &from_label,
            &to_label,
            sort_orders,
        )?;
        tx.commit().map_err(|e| format!("Commit: {e}"))?;
        Ok(changed)
    }

    /// Meeting-scoped retag used by the UI: optionally creates a new
    /// persistent speaker, but only after verifying matching segments exist.
    /// The create + segment rewrite run in one transaction so a failed retag
    /// never leaves an orphan speaker row. When `remember` is true, the voice
    /// embeddings this meeting recorded for `from_persistent_id` move to the
    /// resolved target too, so future matching benefits from the correction;
    /// when false, the retag only overrides the label shown for this meeting
    /// and the embeddings stay where they are. A remembered retag only moves
    /// those embeddings once `from_persistent_id` has no segments left in
    /// this meeting: see the invariant note at the embedding move below.
    pub fn retag_meeting_speaker_labels(
        &self,
        meeting_id: &str,
        from_persistent_id: i64,
        to_speaker_id: Option<i64>,
        new_speaker_name: Option<&str>,
        sort_orders: Option<&[u64]>,
        remember: bool,
    ) -> Result<usize, String> {
        match (to_speaker_id, new_speaker_name) {
            (Some(id), None) => {
                if id == from_persistent_id {
                    return Err("Target speaker must differ from the source speaker".into());
                }
            }
            (None, Some(name)) => {
                if name.trim().is_empty() {
                    return Err("Speaker name cannot be empty".into());
                }
            }
            (Some(_), Some(_)) => {
                return Err("Specify either to_speaker_id or new_speaker_name, not both".into());
            }
            (None, None) => return Err("Specify to_speaker_id or new_speaker_name".into()),
        }

        let from_label = Speaker::Persistent(from_persistent_id).as_str();
        let mut conn = self.conn.acquire()?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction: {e}"))?;

        let candidate_count =
            count_retag_candidates(&tx, meeting_id, &from_label, sort_orders)?;
        if candidate_count == 0 {
            return Err("No matching segments were retagged".into());
        }

        let resolved_to_id = match (to_speaker_id, new_speaker_name) {
            (Some(id), None) => id,
            (None, Some(name)) => {
                let name = name.trim();
                let now = chrono::Utc::now().to_rfc3339();
                tx.execute(
                    "INSERT INTO speakers (name, centroid, embedding_count, created_at, last_seen_at)
                     VALUES (?1, NULL, 0, ?2, ?2)",
                    params![name, now],
                )
                .map_err(|e| format!("Insert speaker: {e}"))?;
                tx.last_insert_rowid()
            }
            _ => unreachable!("validated above"),
        };

        let to_label = Speaker::Persistent(resolved_to_id).as_str();
        let changed = execute_retag_persistent_speaker(
            &tx,
            meeting_id,
            &from_label,
            &to_label,
            sort_orders,
        )?;

        // A meeting's voice embeddings for a speaker cover every turn that
        // speaker took in it, not just the retagged ones: they are only
        // attributable as a block. So a remembered retag may only move them
        // once the source speaker has no segments left in this meeting
        // (whole-meeting retags always satisfy this; a single-turn retag
        // only does when that was the source's sole turn here). Otherwise
        // the source genuinely spoke elsewhere in the meeting and moving its
        // embeddings would mis-train the target's voice profile.
        if remember {
            let remaining_for_source: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM segments WHERE meeting_id = ?1 AND speaker = ?2",
                    params![meeting_id, &from_label],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Count remaining source segments: {e}"))?;
            if remaining_for_source == 0 {
                tx.execute(
                    "UPDATE speaker_embeddings SET speaker_id = ?1
                     WHERE speaker_id = ?2 AND meeting_id = ?3",
                    params![resolved_to_id, from_persistent_id, meeting_id],
                )
                .map_err(|e| format!("Move retagged speaker embeddings: {e}"))?;
                super::speakers::prune_speaker_embeddings(&tx, resolved_to_id)?;
            }
        }

        tx.commit().map_err(|e| format!("Commit: {e}"))?;
        Ok(changed)
    }

    /// Finalize meetings left with `ended_at IS NULL` by a crash mid-recording.
    /// Empty shells (a started meeting with no persisted segments) are deleted;
    /// the rest get `ended_at`/`duration`/`recording_sessions` synthesized from
    /// their persisted segments and are rewritten (which also rebuilds FTS).
    /// Returns the number of meetings salvaged. Safe to call whenever no
    /// recording is live: at startup, and also from the failed-start path in
    /// `launch_meeting`, since the accumulator guard there guarantees no other
    /// meeting is mid-recording when it runs.
    pub fn recover_unfinished_meetings(&self) -> Result<usize, String> {
        let ids: Vec<String> = {
            let conn = self.conn.acquire()?;
            let mut stmt = conn
                .prepare("SELECT id FROM meetings WHERE ended_at IS NULL")
                .map_err(|e| format!("Prepare unfinished: {e}"))?;
            stmt.query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| format!("Query unfinished: {e}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Collect unfinished: {e}"))?
        };

        let mut recovered = 0;
        for id in ids {
            let mut meeting = self.load_meeting(&id)?;
            if meeting.segments.is_empty() {
                self.delete_meeting(&id)?;
                continue;
            }
            let duration = meeting
                .segments
                .iter()
                .map(|s| s.end_time)
                .fold(0.0_f64, f64::max);
            let ended_at =
                meeting.started_at + chrono::Duration::milliseconds((duration * 1000.0) as i64);
            meeting.duration_seconds = duration;
            meeting.ended_at = Some(ended_at);
            if meeting.recording_sessions.is_empty() {
                meeting
                    .recording_sessions
                    .push(MeetingRecordingSession::completed(
                        format!("{id}-recovered"),
                        meeting.started_at,
                        ended_at,
                        0,
                        meeting.segments.len() as u64,
                    ));
            }
            self.save_meeting(&meeting)?;
            recovered += 1;
        }
        Ok(recovered)
    }

    /// Load a full meeting with segments by ID.
    pub fn load_meeting(&self, id: &str) -> Result<MeetingTranscript, String> {
        let conn = self.conn.acquire()?;

        let meeting = conn
            .query_row(
                "SELECT
                    id,
                    title,
                    started_at,
                    ended_at,
                    duration_seconds,
                    transcription_profile,
                    recording_sessions,
                    summary,
                    summary_is_stale,
                    summary_model,
                    summary_generated_at,
                    edited_transcript,
                    notes,
                    calendar_event_id,
                    participants,
                    structured_summary
                 FROM meetings
                 WHERE id = ?1",
                params![id],
                |row| {
                    Ok(MeetingRow {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        started_at: row.get(2)?,
                        ended_at: row.get(3)?,
                        duration_seconds: row.get(4)?,
                        transcription_profile: row.get(5)?,
                        recording_sessions: row.get(6)?,
                        summary: row.get(7)?,
                        summary_is_stale: row.get::<_, i32>(8)? != 0,
                        summary_model: row.get(9)?,
                        summary_generated_at: row.get(10)?,
                        edited_transcript: row.get(11)?,
                        notes: row.get(12)?,
                        calendar_event_id: row.get(13)?,
                        participants: row.get(14)?,
                        structured_summary: row.get(15)?,
                    })
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => format!("Meeting not found: {id}"),
                _ => format!("Query: {e}"),
            })?;

        let mut stmt = conn
            .prepare(
                "SELECT text, start_time, end_time, is_final, language, confidence, speaker
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
                    speaker: row
                        .get::<_, Option<String>>(6)?
                        .as_deref()
                        .and_then(Speaker::parse),
                })
            })
            .map_err(|e| format!("Query segments: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Collect segments: {e}"))?;
        drop(stmt);
        // `speakers_for_meeting` acquires the connection lock itself, so the
        // guard held by this function must be released first (the Mutex is
        // not reentrant).
        drop(conn);

        let transcription_profile = meeting.transcription_profile()?;
        let recording_sessions = meeting.recording_sessions()?;
        let participants = meeting.participants()?;
        let structured_summary = meeting.structured_summary()?;
        let speakers = self
            .speakers_for_meeting(id)?
            .into_iter()
            .map(|record| MeetingSpeaker {
                id: record.id,
                name: record.name,
            })
            .collect();
        let started_at = parse_datetime(&meeting.started_at)?;
        let ended_at = meeting
            .ended_at
            .as_deref()
            .map(parse_datetime)
            .transpose()?;
        let summary_generated_at = meeting
            .summary_generated_at
            .as_deref()
            .map(parse_datetime)
            .transpose()?;

        Ok(MeetingTranscript {
            id: meeting.id,
            title: meeting.title,
            started_at,
            ended_at,
            duration_seconds: meeting.duration_seconds,
            transcription_profile,
            recording_sessions,
            segments,
            summary: meeting.summary,
            summary_is_stale: meeting.summary_is_stale,
            summary_model: meeting.summary_model,
            summary_generated_at,
            edited_transcript: meeting.edited_transcript,
            notes: meeting.notes,
            calendar_event_id: meeting.calendar_event_id,
            participants,
            structured_summary,
            speakers,
        })
    }

    pub fn save_meeting_notes(&self, id: &str, notes: Option<&str>) -> Result<(), String> {
        let conn = self.conn.acquire()?;
        conn.execute(
            "UPDATE meetings SET notes = ?1 WHERE id = ?2",
            params![notes, id],
        )
        .map_err(|e| format!("Update meeting notes: {e}"))?;
        Ok(())
    }

    /// List all meetings (lightweight, no segments).
    pub fn list_meetings(&self) -> Result<Vec<MeetingListItem>, String> {
        let conn = self.conn.acquire()?;

        let mut stmt = conn
            .prepare(
                "SELECT id, title, started_at, duration_seconds, summary IS NOT NULL, summary_is_stale
                 FROM meetings ORDER BY started_at DESC",
            )
            .map_err(|e| format!("Prepare: {e}"))?;

        let items = stmt
            .query_map([], |row| {
                Ok(MeetingListItem {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    started_at: row.get::<_, String>(2).and_then(|value| {
                        DateTime::parse_from_rfc3339(&value)
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
                    summary_is_stale: row.get::<_, i32>(5)? != 0,
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

        conn.execute(
            "DELETE FROM text_search WHERE source_type = 'meeting' AND source_id = ?1",
            params![id],
        )
        .map_err(|e| format!("Delete FTS: {e}"))?;

        conn.execute("DELETE FROM meetings WHERE id = ?1", params![id])
            .map_err(|e| format!("Delete meeting: {e}"))?;

        Ok(())
    }

    /// Update meeting summary fields, structured summary, and clear the stale flag.
    pub fn update_meeting_summary(
        &self,
        id: &str,
        summary: &str,
        structured_summary: Option<&StructuredSummary>,
        model: &str,
    ) -> Result<(), String> {
        let conn = self.conn.acquire()?;
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "UPDATE meetings
             SET summary = ?1, summary_is_stale = 0, summary_model = ?2, summary_generated_at = ?3,
                 structured_summary = ?4
             WHERE id = ?5",
            params![
                summary,
                model,
                now,
                serialize_structured_summary(structured_summary)?,
                id
            ],
        )
        .map_err(|e| format!("Update summary: {e}"))?;

        Ok(())
    }

    /// Save an edited transcript for a meeting.
    pub fn update_meeting_title(&self, id: &str, title: &str) -> Result<(), String> {
        let conn = self.conn.acquire()?;
        conn.execute(
            "UPDATE meetings SET title = ?1 WHERE id = ?2",
            params![title, id],
        )
        .map_err(|e| format!("Update meeting title: {e}"))?;
        Ok(())
    }

    pub fn save_edited_transcript(
        &self,
        id: &str,
        edited_transcript: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.acquire()?;
        conn.execute(
            "UPDATE meetings SET edited_transcript = ?1 WHERE id = ?2",
            params![edited_transcript, id],
        )
        .map_err(|e| format!("Update edited transcript: {e}"))?;
        Ok(())
    }

    /// Load the edited transcript for a meeting.
    pub fn load_edited_transcript(&self, id: &str) -> Result<Option<String>, String> {
        let conn = self.conn.acquire()?;
        conn.query_row(
            "SELECT edited_transcript FROM meetings WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Load edited transcript: {e}"))
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

    /// Total number of meetings, for the Settings > Data stats line.
    pub fn count_meetings(&self) -> Result<u32, String> {
        let conn = self.conn.acquire()?;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM meetings", [], |row| row.get(0))
            .map_err(|e| format!("Count meetings: {e}"))?;
        Ok(count.max(0) as u32)
    }

    /// Test-only: insert a `meetings` row with an unparseable
    /// `transcription_profile`, so `load_meeting` fails on it. Lets the
    /// archive export tests exercise the "skip one bad meeting, keep going"
    /// path without a hand-rolled fake `Database`.
    #[cfg(test)]
    pub fn insert_corrupt_meeting_for_test(&self, id: &str, started_at: &str) -> Result<(), String> {
        let conn = self.conn.acquire()?;
        conn.execute(
            "INSERT INTO meetings (
                id, title, started_at, ended_at, duration_seconds,
                transcription_profile, recording_sessions, summary,
                summary_is_stale, summary_model, summary_generated_at,
                edited_transcript, notes, calendar_event_id, participants
             ) VALUES (?1, 'Corrupt Meeting', ?2, NULL, 0, 'not-json', '[]', NULL, 0, NULL, NULL, NULL, NULL, NULL, NULL)",
            params![id, started_at],
        )
        .map_err(|e| format!("Insert corrupt meeting: {e}"))?;
        Ok(())
    }
}

struct MeetingRow {
    id: String,
    title: String,
    started_at: String,
    ended_at: Option<String>,
    duration_seconds: f64,
    transcription_profile: String,
    recording_sessions: String,
    summary: Option<String>,
    summary_is_stale: bool,
    summary_model: Option<String>,
    summary_generated_at: Option<String>,
    edited_transcript: Option<String>,
    notes: Option<String>,
    calendar_event_id: Option<String>,
    participants: Option<String>,
    structured_summary: Option<String>,
}

impl MeetingRow {
    fn transcription_profile(&self) -> Result<TranscriptionProfile, String> {
        serde_json::from_str(&self.transcription_profile)
            .map_err(|e| format!("Deserialize transcription profile: {e}"))
    }

    fn recording_sessions(&self) -> Result<Vec<MeetingRecordingSession>, String> {
        serde_json::from_str(&self.recording_sessions)
            .map_err(|e| format!("Deserialize recording sessions: {e}"))
    }

    /// NULL (pre-v8 rows) means no participants.
    fn participants(&self) -> Result<Vec<MeetingParticipant>, String> {
        match self.participants.as_deref() {
            Some(raw) => {
                serde_json::from_str(raw).map_err(|e| format!("Deserialize participants: {e}"))
            }
            None => Ok(Vec::new()),
        }
    }

    fn structured_summary(&self) -> Result<Option<StructuredSummary>, String> {
        match self.structured_summary.as_deref() {
            Some(raw) => serde_json::from_str(raw)
                .map(Some)
                .map_err(|e| format!("Deserialize structured summary: {e}")),
            None => Ok(None),
        }
    }
}

/// Participants persist as a JSON array; an empty list stores NULL so pre-v8
/// and participant-less rows look identical.
fn serialize_participants(participants: &[MeetingParticipant]) -> Result<Option<String>, String> {
    if participants.is_empty() {
        return Ok(None);
    }
    serde_json::to_string(participants)
        .map(Some)
        .map_err(|e| format!("Serialize participants: {e}"))
}

fn serialize_structured_summary(
    structured_summary: Option<&StructuredSummary>,
) -> Result<Option<String>, String> {
    match structured_summary {
        Some(value) => serde_json::to_string(value)
            .map(Some)
            .map_err(|e| format!("Serialize structured summary: {e}")),
        None => Ok(None),
    }
}

fn parse_datetime(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| format!("Parse datetime '{value}': {e}"))
}

fn count_retag_candidates(
    tx: &rusqlite::Transaction<'_>,
    meeting_id: &str,
    from_label: &str,
    sort_orders: Option<&[u64]>,
) -> Result<usize, String> {
    match sort_orders {
        None => {
            let count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM segments
                     WHERE meeting_id = ?1 AND speaker = ?2",
                    params![meeting_id, from_label],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Count retag candidates: {e}"))?;
            Ok(count.max(0) as usize)
        }
        Some([]) => Ok(0),
        Some(orders) => {
            let mut total = 0usize;
            for sort_order in orders {
                let count: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM segments
                         WHERE meeting_id = ?1 AND speaker = ?2 AND sort_order = ?3",
                        params![meeting_id, from_label, *sort_order as i64],
                        |row| row.get(0),
                    )
                    .map_err(|e| format!("Count retag candidates: {e}"))?;
                total += count.max(0) as usize;
            }
            Ok(total)
        }
    }
}

fn execute_retag_persistent_speaker(
    tx: &rusqlite::Transaction<'_>,
    meeting_id: &str,
    from_label: &str,
    to_label: &str,
    sort_orders: Option<&[u64]>,
) -> Result<usize, String> {
    match sort_orders {
        None => tx
            .execute(
                "UPDATE segments SET speaker = ?1
                 WHERE meeting_id = ?2 AND speaker = ?3",
                params![to_label, meeting_id, from_label],
            )
            .map_err(|e| format!("Retag meeting speaker: {e}")),
        Some([]) => Ok(0),
        Some(orders) => {
            let mut total = 0usize;
            for sort_order in orders {
                total += tx
                    .execute(
                        "UPDATE segments SET speaker = ?1
                         WHERE meeting_id = ?2 AND sort_order = ?3 AND speaker = ?4",
                        params![to_label, meeting_id, *sort_order as i64, from_label],
                    )
                    .map_err(|e| format!("Retag meeting speaker: {e}"))?;
            }
            Ok(total)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::{Speaker, TranscriptionProfile};
    use crate::test_helpers::fixtures::{sample_meeting, test_db};

    #[test]
    fn load_meeting_resolves_persistent_speakers() {
        let (db, _dir) = test_db();
        let alice = db.create_speaker("Alice").unwrap();
        let bob = db.create_speaker("Bob").unwrap();

        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Persistent(alice));
        meeting.segments[1].speaker = Some(Speaker::Persistent(bob));
        db.save_meeting(&meeting).unwrap();

        let loaded = db.load_meeting("m1").unwrap();
        assert_eq!(loaded.segments[0].speaker, Some(Speaker::Persistent(alice)));
        assert_eq!(loaded.segments[1].speaker, Some(Speaker::Persistent(bob)));
        assert_eq!(
            loaded.speakers.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![alice, bob]
        );
        assert_eq!(loaded.speakers[0].name, "Alice");
        assert_eq!(loaded.speakers[1].name, "Bob");
    }

    #[test]
    fn load_meeting_speakers_empty_for_me_them_only() {
        let (db, _dir) = test_db();
        db.save_meeting(&sample_meeting("m1")).unwrap();
        let loaded = db.load_meeting("m1").unwrap();
        assert!(loaded.speakers.is_empty());
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
        assert_eq!(loaded.recording_sessions.len(), 1);
        assert_eq!(loaded.recording_sessions[0].start_segment_index, 0);
        assert_eq!(loaded.recording_sessions[0].end_segment_index, 2);
        assert_eq!(loaded.segments[0].text, "Hello world");
        assert_eq!(loaded.segments[1].text, "second segment");
        assert_eq!(
            loaded.transcription_profile,
            TranscriptionProfile::default()
        );
    }

    #[test]
    fn meeting_notes_round_trip() {
        let (db, _dir) = test_db();
        let mut meeting = sample_meeting("m1");
        meeting.notes = Some("remember the budget question".to_string());
        db.save_meeting(&meeting).unwrap();

        assert_eq!(
            db.load_meeting("m1").unwrap().notes.as_deref(),
            Some("remember the budget question")
        );

        db.save_meeting_notes("m1", Some("updated notes")).unwrap();
        assert_eq!(
            db.load_meeting("m1").unwrap().notes.as_deref(),
            Some("updated notes")
        );

        db.save_meeting_notes("m1", None).unwrap();
        assert_eq!(db.load_meeting("m1").unwrap().notes, None);
    }

    #[test]
    fn structured_summary_round_trip() {
        use crate::transcript::{StructuredActionItem, StructuredSummary};
        let (db, _dir) = test_db();
        let mut meeting = sample_meeting("m-structured");
        meeting.structured_summary = Some(StructuredSummary {
            decisions: vec!["Ship the feature".to_string()],
            action_items: vec![StructuredActionItem {
                text: "Open PR".to_string(),
                owner: Some("Damien".to_string()),
            }],
            open_questions: vec!["When is Xcode 26 on CI?".to_string()],
        });
        db.save_meeting(&meeting).unwrap();
        let loaded = db.load_meeting("m-structured").unwrap();
        assert_eq!(loaded.structured_summary, meeting.structured_summary);
    }

    #[test]
    fn participants_round_trip_and_survive_resume() {
        use crate::transcript::MeetingParticipant;
        let (db, _dir) = test_db();
        let mut meeting = sample_meeting("m1");
        meeting.calendar_event_id = Some("evt-42".to_string());
        meeting.participants = vec![MeetingParticipant {
            name: "Alice Martin".to_string(),
            email: Some("alice@corp.com".to_string()),
            is_organizer: true,
            is_current_user: false,
        }];
        db.save_meeting(&meeting).unwrap();

        let loaded = db.load_meeting("m1").unwrap();
        assert_eq!(loaded.calendar_event_id.as_deref(), Some("evt-42"));
        assert_eq!(loaded.participants, meeting.participants);

        // Resume path reuses upsert_meeting_header with the loaded meeting.
        let mut header = loaded.clone();
        header.ended_at = None;
        db.upsert_meeting_header(&header).unwrap();
        let resumed = db.load_meeting("m1").unwrap();
        assert_eq!(resumed.participants, meeting.participants);
        assert_eq!(resumed.calendar_event_id.as_deref(), Some("evt-42"));
    }

    #[test]
    fn list_meetings() {
        let (db, _dir) = test_db();
        let first = sample_meeting("m1");
        let mut second = sample_meeting("m2");
        second.summary = Some("Fresh summary".to_string());
        second.summary_is_stale = true;

        db.save_meeting(&first).unwrap();
        db.save_meeting(&second).unwrap();

        let list = db.list_meetings().unwrap();
        assert_eq!(list.len(), 2);
        assert!(
            list.iter()
                .any(|item| item.id == "m2" && item.summary_is_stale)
        );
    }

    #[test]
    fn count_reflects_saved_meetings() {
        let (db, _dir) = test_db();
        assert_eq!(db.count_meetings().unwrap(), 0);

        db.save_meeting(&sample_meeting("m1")).unwrap();
        db.save_meeting(&sample_meeting("m2")).unwrap();
        assert_eq!(db.count_meetings().unwrap(), 2);

        db.delete_meeting("m1").unwrap();
        assert_eq!(db.count_meetings().unwrap(), 1);
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
        use crate::transcript::StructuredSummary;
        let (db, _dir) = test_db();
        let mut meeting = sample_meeting("m1");
        meeting.summary_is_stale = true;
        db.save_meeting(&meeting).unwrap();
        db.update_meeting_summary(
            "m1",
            "Summary text",
            Some(&StructuredSummary {
                decisions: vec!["Go".to_string()],
                action_items: vec![],
                open_questions: vec![],
            }),
            "qwen2.5",
        )
        .unwrap();

        let loaded = db.load_meeting("m1").unwrap();
        assert!(!loaded.summary_is_stale);
        assert_eq!(loaded.summary.as_deref(), Some("Summary text"));
        assert_eq!(loaded.summary_model.as_deref(), Some("qwen2.5"));
        assert!(loaded.summary_generated_at.is_some());
        assert_eq!(loaded.structured_summary.unwrap().decisions, vec!["Go"]);
    }

    #[test]
    fn update_summary_prose_only_clears_structured() {
        use crate::transcript::StructuredSummary;
        let (db, _dir) = test_db();
        db.save_meeting(&sample_meeting("m1")).unwrap();
        db.update_meeting_summary(
            "m1",
            "First pass",
            Some(&StructuredSummary {
                decisions: vec!["Old".to_string()],
                action_items: vec![],
                open_questions: vec![],
            }),
            "qwen2.5",
        )
        .unwrap();
        db.update_meeting_summary("m1", "Prose only after extract fail", None, "qwen2.5")
            .unwrap();

        let loaded = db.load_meeting("m1").unwrap();
        assert_eq!(loaded.summary.as_deref(), Some("Prose only after extract fail"));
        assert!(loaded.structured_summary.is_none());
    }

    #[test]
    fn append_segments_persists_incrementally() {
        use crate::engine::TranscriptionSegment;
        let (db, _dir) = test_db();
        let mut header = sample_meeting("live");
        header.segments.clear();
        header.ended_at = None;
        db.upsert_meeting_header(&header).unwrap();

        let seg = |t: &str, i: f64| TranscriptionSegment {
            text: t.to_string(),
            start_time: i,
            end_time: i + 1.0,
            is_final: true,
            language: None,
            confidence: None,
            speaker: None,
        };

        db.append_segments("live", &[seg("one", 0.0), seg("two", 1.0)], 0)
            .unwrap();
        db.append_segments("live", &[seg("three", 2.0)], 2).unwrap();

        let loaded = db.load_meeting("live").unwrap();
        assert_eq!(loaded.segments.len(), 3);
        assert_eq!(loaded.segments[0].text, "one");
        assert_eq!(loaded.segments[2].text, "three");
        assert!(loaded.ended_at.is_none(), "still in progress");
    }

    #[test]
    fn retag_persistent_speaker_rejects_non_persistent_target() {
        let (db, _dir) = test_db();
        let alice = db.create_speaker("Alice").unwrap();

        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Persistent(alice));
        db.save_meeting(&meeting).unwrap();

        assert_eq!(
            db.retag_persistent_speaker("m1", alice, Speaker::Me, None)
                .unwrap_err(),
            "Retag target must be a persistent speaker"
        );
        assert_eq!(
            db.retag_persistent_speaker("m1", alice, Speaker::Them, None)
                .unwrap_err(),
            "Retag target must be a persistent speaker"
        );
    }

    #[test]
    fn retag_meeting_speaker_labels_rejects_empty_new_name() {
        let (db, _dir) = test_db();
        let alice = db.create_speaker("Alice").unwrap();

        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Persistent(alice));
        db.save_meeting(&meeting).unwrap();

        assert_eq!(
            db.retag_meeting_speaker_labels("m1", alice, None, Some("  "), None, false)
                .unwrap_err(),
            "Speaker name cannot be empty"
        );
    }

    #[test]
    fn retag_meeting_speaker_labels_does_not_create_speaker_without_matches() {
        let (db, _dir) = test_db();
        let alice = db.create_speaker("Alice").unwrap();
        db.save_meeting(&sample_meeting("m1")).unwrap();

        assert_eq!(
            db.retag_meeting_speaker_labels("m1", alice, None, Some("Carol"), None, false)
                .unwrap_err(),
            "No matching segments were retagged"
        );
        assert_eq!(db.list_speakers().unwrap().len(), 1);
    }

    #[test]
    fn retag_meeting_speaker_labels_creates_speaker_only_when_segments_match() {
        let (db, _dir) = test_db();
        let alice = db.create_speaker("Alice").unwrap();

        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Persistent(alice));
        db.save_meeting(&meeting).unwrap();

        let changed = db
            .retag_meeting_speaker_labels("m1", alice, None, Some("Carol"), None, false)
            .unwrap();
        assert_eq!(changed, 1);

        let speakers = db.list_speakers().unwrap();
        assert_eq!(speakers.len(), 2);
        let carol = speakers.iter().find(|speaker| speaker.name == "Carol").unwrap();

        let loaded = db.load_meeting("m1").unwrap();
        assert_eq!(
            loaded.segments[0].speaker,
            Some(Speaker::Persistent(carol.id))
        );
    }

    #[test]
    fn retag_meeting_speaker_labels_remember_moves_meeting_embeddings_to_target() {
        let (db, _dir) = test_db();
        let alice = db.create_speaker("Alice").unwrap();
        let bob = db.create_speaker("Bob").unwrap();

        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Persistent(alice));
        db.save_meeting(&meeting).unwrap();

        // One embedding from this meeting, one from another: only the one
        // tied to this meeting should move on a remembered retag.
        db.append_speaker_embedding(alice, Some("m1"), &[1u8], 1.0, chrono::Utc::now())
            .unwrap();
        db.append_speaker_embedding(alice, Some("m2"), &[2u8], 1.0, chrono::Utc::now())
            .unwrap();

        db.retag_meeting_speaker_labels("m1", alice, Some(bob), None, None, true)
            .unwrap();

        assert_eq!(db.speaker_embeddings(alice).unwrap(), vec![vec![2u8]]);
        assert_eq!(db.speaker_embeddings(bob).unwrap(), vec![vec![1u8]]);
    }

    #[test]
    fn retag_meeting_speaker_labels_without_remember_leaves_embeddings_in_place() {
        let (db, _dir) = test_db();
        let alice = db.create_speaker("Alice").unwrap();
        let bob = db.create_speaker("Bob").unwrap();

        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Persistent(alice));
        db.save_meeting(&meeting).unwrap();
        db.append_speaker_embedding(alice, Some("m1"), &[1u8], 1.0, chrono::Utc::now())
            .unwrap();

        db.retag_meeting_speaker_labels("m1", alice, Some(bob), None, None, false)
            .unwrap();

        assert_eq!(db.speaker_embeddings(alice).unwrap(), vec![vec![1u8]]);
        assert!(db.speaker_embeddings(bob).unwrap().is_empty());
    }

    #[test]
    fn retag_meeting_speaker_labels_remember_leaves_embeddings_when_source_keeps_other_turns() {
        let (db, _dir) = test_db();
        let alice = db.create_speaker("Alice").unwrap();
        let bob = db.create_speaker("Bob").unwrap();

        // Alice has two turns in this meeting; only one is retagged, so she
        // still spoke in the meeting after the correction.
        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Persistent(alice));
        meeting.segments[1].speaker = Some(Speaker::Persistent(alice));
        db.save_meeting(&meeting).unwrap();
        db.append_speaker_embedding(alice, Some("m1"), &[1u8], 1.0, chrono::Utc::now())
            .unwrap();

        db.retag_meeting_speaker_labels("m1", alice, Some(bob), None, Some(&[0]), true)
            .unwrap();

        let loaded = db.load_meeting("m1").unwrap();
        assert_eq!(loaded.segments[0].speaker, Some(Speaker::Persistent(bob)));
        assert_eq!(loaded.segments[1].speaker, Some(Speaker::Persistent(alice)));

        // Alice's meeting embeddings stay put: she is still attributable to
        // segment 1, so moving them would mis-train Bob's voice profile.
        assert_eq!(db.speaker_embeddings(alice).unwrap(), vec![vec![1u8]]);
        assert!(db.speaker_embeddings(bob).unwrap().is_empty());
    }

    #[test]
    fn retag_meeting_speaker_labels_remember_moves_embeddings_when_turn_retag_fully_delogs_source()
    {
        let (db, _dir) = test_db();
        let alice = db.create_speaker("Alice").unwrap();
        let bob = db.create_speaker("Bob").unwrap();

        // Alice only has a single turn in this meeting (segment 0); segment 1
        // belongs to Bob. Retagging Alice's sole turn fully delogs her from
        // the meeting, so the turn-scoped retag behaves like a meeting-scoped
        // one for embedding purposes.
        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Persistent(alice));
        meeting.segments[1].speaker = Some(Speaker::Persistent(bob));
        db.save_meeting(&meeting).unwrap();
        db.append_speaker_embedding(alice, Some("m1"), &[1u8], 1.0, chrono::Utc::now())
            .unwrap();

        db.retag_meeting_speaker_labels("m1", alice, Some(bob), None, Some(&[0]), true)
            .unwrap();

        let loaded = db.load_meeting("m1").unwrap();
        assert_eq!(loaded.segments[0].speaker, Some(Speaker::Persistent(bob)));

        assert!(db.speaker_embeddings(alice).unwrap().is_empty());
        assert_eq!(db.speaker_embeddings(bob).unwrap(), vec![vec![1u8]]);
    }

    #[test]
    fn retag_meeting_speaker_labels_remember_only_moves_this_meetings_rows_for_new_speaker() {
        let (db, _dir) = test_db();
        let alice = db.create_speaker("Alice").unwrap();

        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Persistent(alice));
        db.save_meeting(&meeting).unwrap();
        db.append_speaker_embedding(alice, Some("m1"), &[1u8], 1.0, chrono::Utc::now())
            .unwrap();
        db.append_speaker_embedding(alice, Some("m2"), &[2u8], 1.0, chrono::Utc::now())
            .unwrap();

        let changed = db
            .retag_meeting_speaker_labels("m1", alice, None, Some("Carol"), None, true)
            .unwrap();
        assert_eq!(changed, 1);

        let carol = db
            .list_speakers()
            .unwrap()
            .into_iter()
            .find(|speaker| speaker.name == "Carol")
            .unwrap();

        assert_eq!(db.speaker_embeddings(alice).unwrap(), vec![vec![2u8]]);
        assert_eq!(db.speaker_embeddings(carol.id).unwrap(), vec![vec![1u8]]);
    }

    #[test]
    fn retag_persistent_speaker_rewrites_matching_segments() {
        let (db, _dir) = test_db();
        let alice = db.create_speaker("Alice").unwrap();
        let bob = db.create_speaker("Bob").unwrap();

        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Persistent(alice));
        meeting.segments[1].speaker = Some(Speaker::Persistent(alice));
        db.save_meeting(&meeting).unwrap();

        let changed = db
            .retag_persistent_speaker("m1", alice, Speaker::Persistent(bob), None)
            .unwrap();
        assert_eq!(changed, 2);

        let loaded = db.load_meeting("m1").unwrap();
        assert_eq!(loaded.segments[0].speaker, Some(Speaker::Persistent(bob)));
        assert_eq!(loaded.segments[1].speaker, Some(Speaker::Persistent(bob)));
    }

    #[test]
    fn retag_persistent_speaker_leaves_me_and_them_untouched() {
        let (db, _dir) = test_db();
        let alice = db.create_speaker("Alice").unwrap();
        let bob = db.create_speaker("Bob").unwrap();

        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Persistent(alice));
        meeting.segments[1].speaker = Some(Speaker::Me);
        db.save_meeting(&meeting).unwrap();

        let changed = db
            .retag_persistent_speaker("m1", alice, Speaker::Persistent(bob), None)
            .unwrap();
        assert_eq!(changed, 1);

        let loaded = db.load_meeting("m1").unwrap();
        assert_eq!(loaded.segments[0].speaker, Some(Speaker::Persistent(bob)));
        assert_eq!(loaded.segments[1].speaker, Some(Speaker::Me));
    }

    #[test]
    fn retag_persistent_speaker_scoped_to_sort_orders() {
        let (db, _dir) = test_db();
        let alice = db.create_speaker("Alice").unwrap();
        let bob = db.create_speaker("Bob").unwrap();

        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Persistent(alice));
        meeting.segments[1].speaker = Some(Speaker::Persistent(alice));
        db.save_meeting(&meeting).unwrap();

        let changed = db
            .retag_persistent_speaker("m1", alice, Speaker::Persistent(bob), Some(&[0]))
            .unwrap();
        assert_eq!(changed, 1);

        let loaded = db.load_meeting("m1").unwrap();
        assert_eq!(loaded.segments[0].speaker, Some(Speaker::Persistent(bob)));
        assert_eq!(loaded.segments[1].speaker, Some(Speaker::Persistent(alice)));
    }

    #[test]
    fn retag_persistent_speaker_does_not_touch_me_or_them() {
        let (db, _dir) = test_db();
        let alice = db.create_speaker("Alice").unwrap();

        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Me);
        meeting.segments[1].speaker = Some(Speaker::Them);
        db.save_meeting(&meeting).unwrap();

        let changed = db
            .retag_persistent_speaker("m1", alice, Speaker::Persistent(99), None)
            .unwrap();
        assert_eq!(changed, 0);
    }

    #[test]
    fn set_segment_speakers_without_replaceable_only_rewrites_null_speakers() {
        let (db, _dir) = test_db();
        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Me);
        meeting.segments[1].speaker = None;
        db.save_meeting(&meeting).unwrap();

        let changed = db
            .set_segment_speakers(
                "m1",
                &[(0, Speaker::Persistent(7)), (1, Speaker::Persistent(7))],
                None,
            )
            .unwrap();
        assert_eq!(changed, 1, "the Me segment must not be rewritten");

        let loaded = db.load_meeting("m1").unwrap();
        assert_eq!(loaded.segments[0].speaker, Some(Speaker::Me));
        assert_eq!(loaded.segments[1].speaker, Some(Speaker::Persistent(7)));
    }

    /// Regression test for the system-audio diarization pass: segments in a
    /// meeting with a system-audio lane are live-labeled Me/Them, and each
    /// offline pass must be able to replace exactly its own lane.
    #[test]
    fn set_segment_speakers_replaces_only_the_allowed_lane() {
        let (db, _dir) = test_db();
        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Me);
        meeting.segments[1].speaker = Some(Speaker::Them);
        db.save_meeting(&meeting).unwrap();

        // System pass: may replace Them, must not touch Me.
        let changed = db
            .set_segment_speakers(
                "m1",
                &[(0, Speaker::Persistent(7)), (1, Speaker::Persistent(7))],
                Some(Speaker::Them),
            )
            .unwrap();
        assert_eq!(changed, 1, "only the Them segment is replaceable");

        let loaded = db.load_meeting("m1").unwrap();
        assert_eq!(loaded.segments[0].speaker, Some(Speaker::Me));
        assert_eq!(loaded.segments[1].speaker, Some(Speaker::Persistent(7)));

        // Mic pass: may replace Me, must not touch the persistent label the
        // system pass just wrote.
        let changed = db
            .set_segment_speakers(
                "m1",
                &[(0, Speaker::Persistent(9)), (1, Speaker::Persistent(9))],
                Some(Speaker::Me),
            )
            .unwrap();
        assert_eq!(changed, 1, "only the Me segment is replaceable");

        let loaded = db.load_meeting("m1").unwrap();
        assert_eq!(loaded.segments[0].speaker, Some(Speaker::Persistent(9)));
        assert_eq!(loaded.segments[1].speaker, Some(Speaker::Persistent(7)));
    }

    #[test]
    fn set_segment_speakers_rejects_persistent_replaceable() {
        let (db, _dir) = test_db();
        db.save_meeting(&sample_meeting("m1")).unwrap();
        let result = db.set_segment_speakers(
            "m1",
            &[(0, Speaker::Persistent(1))],
            Some(Speaker::Persistent(2)),
        );
        assert!(result.is_err());
    }

    #[test]
    fn set_segment_speakers_empty_assignments_is_a_noop() {
        let (db, _dir) = test_db();
        db.save_meeting(&sample_meeting("m1")).unwrap();
        assert_eq!(db.set_segment_speakers("m1", &[], None).unwrap(), 0);
    }

    #[test]
    fn set_segment_speakers_ignores_unknown_sort_orders() {
        let (db, _dir) = test_db();
        db.save_meeting(&sample_meeting("m1")).unwrap();
        let changed = db
            .set_segment_speakers("m1", &[(999, Speaker::Persistent(1))], None)
            .unwrap();
        assert_eq!(changed, 0);
    }

    #[test]
    fn set_segment_speakers_keeps_fts_search_working() {
        let (db, _dir) = test_db();
        db.save_meeting(&sample_meeting("m1")).unwrap();
        db.set_segment_speakers("m1", &[(0, Speaker::Persistent(1))], None)
            .unwrap();
        let hits = db.search_text("Hello", 10).unwrap();
        assert!(hits.iter().any(|hit| hit.source_id == "m1"));
    }

    #[test]
    fn recover_unfinished_finalizes_and_prunes() {
        use crate::engine::TranscriptionSegment;
        let (db, _dir) = test_db();

        // A meeting with live-appended segments but no ended_at (crash).
        let mut crashed = sample_meeting("crashed");
        crashed.segments.clear();
        crashed.ended_at = None;
        crashed.recording_sessions.clear();
        db.upsert_meeting_header(&crashed).unwrap();
        db.append_segments(
            "crashed",
            &[TranscriptionSegment {
                text: "salvage me".to_string(),
                start_time: 0.0,
                end_time: 12.0,
                is_final: true,
                language: None,
                confidence: None,
                speaker: None,
            }],
            0,
        )
        .unwrap();

        // An empty shell (started, no segments, crash) — should be pruned.
        let mut empty = sample_meeting("empty");
        empty.segments.clear();
        empty.ended_at = None;
        db.upsert_meeting_header(&empty).unwrap();

        let recovered = db.recover_unfinished_meetings().unwrap();
        assert_eq!(recovered, 1);

        let salvaged = db.load_meeting("crashed").unwrap();
        assert!(salvaged.ended_at.is_some());
        assert_eq!(salvaged.duration_seconds, 12.0);
        assert_eq!(salvaged.recording_sessions.len(), 1);
        assert_eq!(salvaged.segments.len(), 1);

        assert!(!db.meeting_exists("empty").unwrap(), "empty shell pruned");
    }

    #[test]
    fn upsert_header_preserves_edited_transcript_on_resume() {
        let (db, _dir) = test_db();
        db.save_meeting(&sample_meeting("m1")).unwrap();
        db.save_edited_transcript("m1", Some("my edits")).unwrap();

        // Resume reuses upsert_meeting_header — edits must survive.
        let mut header = sample_meeting("m1");
        header.ended_at = None;
        db.upsert_meeting_header(&header).unwrap();

        let loaded = db.load_meeting("m1").unwrap();
        assert_eq!(loaded.edited_transcript.as_deref(), Some("my edits"));
        assert!(loaded.ended_at.is_none());
    }

    #[test]
    fn load_nonexistent_meeting_returns_error() {
        let (db, _dir) = test_db();
        assert!(db.load_meeting("nonexistent").is_err());
    }

    #[test]
    fn save_and_load_edited_transcript() {
        let (db, _dir) = test_db();
        db.save_meeting(&sample_meeting("m1")).unwrap();

        // Initially no edited transcript
        let edited = db.load_edited_transcript("m1").unwrap();
        assert!(edited.is_none());

        // Save edited transcript
        db.save_edited_transcript("m1", Some("Edited version of transcript"))
            .unwrap();
        let edited = db.load_edited_transcript("m1").unwrap();
        assert_eq!(edited.as_deref(), Some("Edited version of transcript"));

        // Verify load_meeting also returns it
        let meeting = db.load_meeting("m1").unwrap();
        assert_eq!(
            meeting.edited_transcript.as_deref(),
            Some("Edited version of transcript")
        );

        // Clear edited transcript
        db.save_edited_transcript("m1", None).unwrap();
        let edited = db.load_edited_transcript("m1").unwrap();
        assert!(edited.is_none());
    }
}
