use chrono::{DateTime, Utc};
use rusqlite::params;

use crate::lock_ext::MutexExt;

use super::Database;

/// A persistent, cross-meeting speaker identity row. `centroid` is an opaque
/// BLOB from this layer's point of view (the diarization crate encodes it as
/// 256 little-endian f32s); it is `None` until at least one embedding has
/// been recorded for this speaker.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerRecord {
    pub id: i64,
    pub name: String,
    pub centroid: Option<Vec<u8>>,
    pub embedding_count: i64,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

/// How many transcript segments and distinct meetings reference a speaker,
/// for the Settings management list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpeakerUsage {
    pub segment_count: i64,
    pub meeting_count: i64,
}

/// A stored speaker together with its recent voice embeddings (raw BLOBs,
/// undecoded), for the offline diarization matcher.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerWithEmbeddings {
    pub speaker: SpeakerRecord,
    pub embeddings: Vec<Vec<u8>>,
}

/// How many recent embeddings `append_speaker_embedding` keeps per speaker.
/// A bag of embeddings (matched by MAX cosine similarity) tolerates
/// variation across sessions, mics, and vocal tone far better than a single
/// running-mean centroid, but an unbounded bag would grow forever and slow
/// matching down; 20 is enough recent samples to cover that variation
/// without a meaningful cost per match.
pub const MAX_EMBEDDINGS_PER_SPEAKER: usize = 20;

impl Database {
    /// Create a new persistent speaker with the given display name. Returns
    /// the new row's id. `centroid`/`embedding_count` are unused legacy
    /// columns (kept for schema stability, see `schema::CREATE_SPEAKERS`)
    /// and stay at their empty defaults; call `append_speaker_embedding`
    /// once diarization has an embedding for it.
    pub fn create_speaker(&self, name: &str) -> Result<i64, String> {
        let conn = self.conn.acquire()?;
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO speakers (name, centroid, embedding_count, created_at, last_seen_at)
             VALUES (?1, NULL, 0, ?2, ?2)",
            params![name, now],
        )
        .map_err(|e| format!("Insert speaker: {e}"))?;
        Ok(conn.last_insert_rowid())
    }

    /// Look up a single speaker by id. `None` if it doesn't exist (e.g. it
    /// was deleted after a meeting's segments referenced it).
    pub fn get_speaker(&self, id: i64) -> Result<Option<SpeakerRecord>, String> {
        let conn = self.conn.acquire()?;
        let row = conn
            .query_row(
                "SELECT id, name, centroid, embedding_count, created_at, last_seen_at
                 FROM speakers WHERE id = ?1",
                params![id],
                map_speaker_row,
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(format!("Query speaker: {other}")),
            })?;
        row.map(SpeakerRow::into_record).transpose()
    }

    /// All persistent speakers, ordered by id.
    pub fn list_speakers(&self) -> Result<Vec<SpeakerRecord>, String> {
        let conn = self.conn.acquire()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, centroid, embedding_count, created_at, last_seen_at
                 FROM speakers ORDER BY id",
            )
            .map_err(|e| format!("Prepare list speakers: {e}"))?;
        let rows = stmt
            .query_map([], map_speaker_row)
            .map_err(|e| format!("Query list speakers: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Collect list speakers: {e}"))?;
        rows.into_iter().map(SpeakerRow::into_record).collect()
    }

    /// Update a persistent speaker's display name. Propagates everywhere the
    /// speaker id appears because labels are `spk:<id>` and names are read
    /// from this table at meeting load time.
    pub fn rename_speaker(&self, id: i64, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Speaker name cannot be empty".into());
        }
        let conn = self.conn.acquire()?;
        let changed = conn
            .execute(
                "UPDATE speakers SET name = ?1 WHERE id = ?2",
                params![name, id],
            )
            .map_err(|e| format!("Rename speaker: {e}"))?;
        if changed == 0 {
            return Err(format!("Speaker not found: {id}"));
        }
        Ok(())
    }

    /// Record one new voice embedding for a speaker (from a matched or
    /// freshly enrolled diarization cluster), touch its `last_seen_at`, and
    /// prune down to `MAX_EMBEDDINGS_PER_SPEAKER` by dropping the oldest
    /// rows (by `created_at`, then `id` to break ties deterministically).
    /// One transaction, so a crash mid-append never leaves the speaker with
    /// more than the cap or a stale `last_seen_at`.
    pub fn append_speaker_embedding(
        &self,
        speaker_id: i64,
        meeting_id: Option<&str>,
        embedding: &[u8],
        speech_seconds: f64,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        let now = now.to_rfc3339();
        let mut conn = self.conn.acquire()?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction: {e}"))?;

        tx.execute(
            "INSERT INTO speaker_embeddings (speaker_id, meeting_id, embedding, speech_seconds, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![speaker_id, meeting_id, embedding, speech_seconds, now],
        )
        .map_err(|e| format!("Insert speaker embedding: {e}"))?;

        tx.execute(
            "DELETE FROM speaker_embeddings WHERE speaker_id = ?1 AND id NOT IN (
                SELECT id FROM speaker_embeddings WHERE speaker_id = ?1
                ORDER BY created_at DESC, id DESC LIMIT ?2
             )",
            params![speaker_id, MAX_EMBEDDINGS_PER_SPEAKER as i64],
        )
        .map_err(|e| format!("Prune speaker embeddings: {e}"))?;

        tx.execute(
            "UPDATE speakers SET last_seen_at = ?1 WHERE id = ?2",
            params![now, speaker_id],
        )
        .map_err(|e| format!("Update speaker last_seen_at: {e}"))?;

        tx.commit().map_err(|e| format!("Commit: {e}"))?;
        Ok(())
    }

    /// Raw embedding BLOBs for one speaker, oldest first. Callers decode
    /// with `diarize::persist::decode_embedding`.
    pub fn speaker_embeddings(&self, speaker_id: i64) -> Result<Vec<Vec<u8>>, String> {
        let conn = self.conn.acquire()?;
        let mut stmt = conn
            .prepare("SELECT embedding FROM speaker_embeddings WHERE speaker_id = ?1 ORDER BY id")
            .map_err(|e| format!("Prepare speaker embeddings: {e}"))?;
        stmt.query_map(params![speaker_id], |row| row.get::<_, Vec<u8>>(0))
            .map_err(|e| format!("Query speaker embeddings: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Collect speaker embeddings: {e}"))
    }

    /// Every persistent speaker with its embedding BLOBs, for the offline
    /// diarization matcher. One query for all embeddings (grouped in Rust)
    /// rather than N+1 per-speaker queries.
    pub fn list_speakers_with_embeddings(&self) -> Result<Vec<SpeakerWithEmbeddings>, String> {
        let speakers = self.list_speakers()?;

        let mut by_speaker: std::collections::HashMap<i64, Vec<Vec<u8>>> =
            std::collections::HashMap::new();
        {
            let conn = self.conn.acquire()?;
            let mut stmt = conn
                .prepare("SELECT speaker_id, embedding FROM speaker_embeddings ORDER BY speaker_id, id")
                .map_err(|e| format!("Prepare list embeddings: {e}"))?;
            let rows = stmt
                .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)))
                .map_err(|e| format!("Query list embeddings: {e}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Collect list embeddings: {e}"))?;
            for (speaker_id, embedding) in rows {
                by_speaker.entry(speaker_id).or_default().push(embedding);
            }
        }

        Ok(speakers
            .into_iter()
            .map(|speaker| {
                let embeddings = by_speaker.remove(&speaker.id).unwrap_or_default();
                SpeakerWithEmbeddings { speaker, embeddings }
            })
            .collect())
    }

    /// Usage counts for every persistent speaker, keyed by speaker id.
    /// Speakers referenced by no segment map to zero counts.
    pub fn speaker_usage(&self) -> Result<std::collections::HashMap<i64, SpeakerUsage>, String> {
        let conn = self.conn.acquire()?;
        let mut stmt = conn
            .prepare(
                "SELECT s.id, COUNT(seg.rowid), COUNT(DISTINCT seg.meeting_id)
                 FROM speakers s
                 LEFT JOIN segments seg ON seg.speaker = 'spk:' || s.id
                 GROUP BY s.id",
            )
            .map_err(|e| format!("Prepare speaker usage: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    SpeakerUsage {
                        segment_count: row.get(1)?,
                        meeting_count: row.get(2)?,
                    },
                ))
            })
            .map_err(|e| format!("Query speaker usage: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Collect speaker usage: {e}"))?;
        Ok(rows.into_iter().collect())
    }

    /// Delete a persistent speaker, its recorded embeddings, and unlabel
    /// every segment that referenced it (their speaker returns to NULL, so
    /// the lines render as plain unattributed text). One transaction, so a
    /// failure never leaves segments pointing at a missing speaker row or
    /// orphaned `speaker_embeddings` rows behind (no FK cascade covers this;
    /// see `schema::CREATE_SPEAKER_EMBEDDINGS`).
    pub fn delete_speaker(&self, id: i64) -> Result<(), String> {
        let label = crate::engine::Speaker::Persistent(id).as_str();
        let mut conn = self.conn.acquire()?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction: {e}"))?;
        tx.execute(
            "UPDATE segments SET speaker = NULL WHERE speaker = ?1",
            params![label],
        )
        .map_err(|e| format!("Unlabel speaker segments: {e}"))?;
        tx.execute(
            "DELETE FROM speaker_embeddings WHERE speaker_id = ?1",
            params![id],
        )
        .map_err(|e| format!("Delete speaker embeddings: {e}"))?;
        let deleted = tx
            .execute("DELETE FROM speakers WHERE id = ?1", params![id])
            .map_err(|e| format!("Delete speaker: {e}"))?;
        if deleted == 0 {
            return Err(format!("Speaker not found: {id}"));
        }
        tx.commit().map_err(|e| format!("Commit: {e}"))?;
        Ok(())
    }

    /// Persistent speakers referenced by any segment of `meeting_id`
    /// (`speaker` column values of the form `spk:<id>`), ordered by id. A
    /// referenced id that no longer has a `speakers` row (deleted) is
    /// silently skipped; callers fall back to a "Speaker <id>" label.
    pub fn speakers_for_meeting(&self, meeting_id: &str) -> Result<Vec<SpeakerRecord>, String> {
        let ids: Vec<i64> = {
            let conn = self.conn.acquire()?;
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT speaker FROM segments
                     WHERE meeting_id = ?1 AND speaker LIKE 'spk:%'",
                )
                .map_err(|e| format!("Prepare speakers for meeting: {e}"))?;
            stmt.query_map(params![meeting_id], |row| row.get::<_, String>(0))
                .map_err(|e| format!("Query speakers for meeting: {e}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Collect speakers for meeting: {e}"))?
                .into_iter()
                .filter_map(|raw| match crate::engine::Speaker::parse(&raw) {
                    Some(crate::engine::Speaker::Persistent(id)) => Some(id),
                    _ => None,
                })
                .collect()
        };

        let mut records = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(record) = self.get_speaker(id)? {
                records.push(record);
            }
        }
        records.sort_by_key(|record| record.id);
        Ok(records)
    }
}

/// Raw row shape before datetime parsing, so parse errors can be reported
/// with a consistent "Parse <field>" message.
struct SpeakerRow {
    id: i64,
    name: String,
    centroid: Option<Vec<u8>>,
    embedding_count: i64,
    created_at: String,
    last_seen_at: String,
}

impl SpeakerRow {
    fn into_record(self) -> Result<SpeakerRecord, String> {
        Ok(SpeakerRecord {
            id: self.id,
            name: self.name,
            centroid: self.centroid,
            embedding_count: self.embedding_count,
            created_at: parse_datetime(&self.created_at)?,
            last_seen_at: parse_datetime(&self.last_seen_at)?,
        })
    }
}

fn map_speaker_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SpeakerRow> {
    Ok(SpeakerRow {
        id: row.get(0)?,
        name: row.get(1)?,
        centroid: row.get(2)?,
        embedding_count: row.get(3)?,
        created_at: row.get(4)?,
        last_seen_at: row.get(5)?,
    })
}

fn parse_datetime(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| format!("Parse datetime '{value}': {e}"))
}

#[cfg(test)]
mod tests {
    use rusqlite::params;

    use crate::engine::Speaker;
    use crate::test_helpers::fixtures::{sample_meeting, test_db};

    #[test]
    fn create_and_get_speaker_round_trips() {
        let (db, _dir) = test_db();
        let id = db.create_speaker("Alice").unwrap();

        let record = db.get_speaker(id).unwrap().expect("speaker exists");
        assert_eq!(record.name, "Alice");
        assert_eq!(record.embedding_count, 0);
        assert!(record.centroid.is_none());
        assert_eq!(record.created_at, record.last_seen_at);
    }

    #[test]
    fn get_speaker_missing_id_returns_none() {
        let (db, _dir) = test_db();
        assert!(db.get_speaker(999).unwrap().is_none());
    }

    #[test]
    fn list_speakers_orders_by_id() {
        let (db, _dir) = test_db();
        let a = db.create_speaker("Alice").unwrap();
        let b = db.create_speaker("Bob").unwrap();

        let all = db.list_speakers().unwrap();
        assert_eq!(all.iter().map(|s| s.id).collect::<Vec<_>>(), vec![a, b]);
        assert_eq!(all[0].name, "Alice");
        assert_eq!(all[1].name, "Bob");
    }

    #[test]
    fn rename_speaker_updates_display_name() {
        let (db, _dir) = test_db();
        let id = db.create_speaker("Alice").unwrap();
        db.rename_speaker(id, "Alicia").unwrap();
        let record = db.get_speaker(id).unwrap().expect("speaker exists");
        assert_eq!(record.name, "Alicia");
    }

    #[test]
    fn rename_speaker_rejects_empty_name() {
        let (db, _dir) = test_db();
        let id = db.create_speaker("Alice").unwrap();
        assert!(db.rename_speaker(id, "  ").is_err());
    }

    #[test]
    fn rename_speaker_missing_id_errors() {
        let (db, _dir) = test_db();
        assert!(db.rename_speaker(999, "Bob").is_err());
    }

    #[test]
    fn append_speaker_embedding_round_trips_and_touches_last_seen_at() {
        let (db, _dir) = test_db();
        let id = db.create_speaker("Alice").unwrap();
        let embedding = vec![0u8; 1024]; // 256 f32s
        let now = chrono::Utc::now();

        db.append_speaker_embedding(id, Some("m1"), &embedding, 6.5, now)
            .unwrap();

        let stored = db.speaker_embeddings(id).unwrap();
        assert_eq!(stored, vec![embedding]);

        let record = db.get_speaker(id).unwrap().unwrap();
        // Sub-second precision may be lost in the RFC3339 round trip; compare
        // at second resolution.
        assert_eq!(record.last_seen_at.timestamp(), now.timestamp());
    }

    #[test]
    fn append_speaker_embedding_caps_at_max_dropping_oldest() {
        let (db, _dir) = test_db();
        let id = db.create_speaker("Alice").unwrap();
        let base = chrono::Utc::now();

        // One more than the cap; each embedding is a distinguishable single
        // byte so we can tell which ones survived.
        for i in 0..(super::MAX_EMBEDDINGS_PER_SPEAKER + 1) {
            let embedding = vec![i as u8];
            let now = base + chrono::Duration::seconds(i as i64);
            db.append_speaker_embedding(id, None, &embedding, 1.0, now)
                .unwrap();
        }

        let stored = db.speaker_embeddings(id).unwrap();
        assert_eq!(stored.len(), super::MAX_EMBEDDINGS_PER_SPEAKER);
        // The oldest (embedding 0) must have been pruned; the newest
        // (embedding MAX_EMBEDDINGS_PER_SPEAKER) must remain.
        assert!(!stored.contains(&vec![0u8]));
        assert!(stored.contains(&vec![super::MAX_EMBEDDINGS_PER_SPEAKER as u8]));
    }

    #[test]
    fn list_speakers_with_embeddings_groups_by_speaker() {
        let (db, _dir) = test_db();
        let alice = db.create_speaker("Alice").unwrap();
        let bob = db.create_speaker("Bob").unwrap();
        let now = chrono::Utc::now();

        db.append_speaker_embedding(alice, None, &[1u8], 1.0, now)
            .unwrap();
        db.append_speaker_embedding(alice, None, &[2u8], 1.0, now)
            .unwrap();

        let all = db.list_speakers_with_embeddings().unwrap();
        let alice_entry = all.iter().find(|s| s.speaker.id == alice).unwrap();
        let bob_entry = all.iter().find(|s| s.speaker.id == bob).unwrap();
        assert_eq!(alice_entry.embeddings, vec![vec![1u8], vec![2u8]]);
        assert!(bob_entry.embeddings.is_empty());
    }

    #[test]
    fn delete_speaker_removes_its_embeddings() {
        let (db, _dir) = test_db();
        let id = db.create_speaker("Alice").unwrap();
        db.append_speaker_embedding(id, None, &[1u8], 1.0, chrono::Utc::now())
            .unwrap();

        db.delete_speaker(id).unwrap();

        let conn = db.conn.lock().unwrap();
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM speaker_embeddings WHERE speaker_id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0, "deleting a speaker must remove its embedding rows");
    }

    #[test]
    fn speakers_for_meeting_resolves_distinct_persistent_ids() {
        let (db, _dir) = test_db();
        let alice = db.create_speaker("Alice").unwrap();
        let bob = db.create_speaker("Bob").unwrap();

        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Persistent(alice));
        meeting.segments[1].speaker = Some(Speaker::Persistent(bob));
        db.save_meeting(&meeting).unwrap();

        let speakers = db.speakers_for_meeting("m1").unwrap();
        assert_eq!(
            speakers.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![alice, bob]
        );
    }

    #[test]
    fn speakers_for_meeting_ignores_me_and_them() {
        let (db, _dir) = test_db();
        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Me);
        meeting.segments[1].speaker = Some(Speaker::Them);
        db.save_meeting(&meeting).unwrap();

        assert!(db.speakers_for_meeting("m1").unwrap().is_empty());
    }

    #[test]
    fn speakers_for_meeting_skips_deleted_speaker_rows() {
        let (db, _dir) = test_db();
        // A segment references a persistent speaker id that was never (or no
        // longer is) a row in `speakers` — should be silently dropped, not
        // error, so the caller's fallback label kicks in instead.
        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Persistent(4242));
        db.save_meeting(&meeting).unwrap();

        assert!(db.speakers_for_meeting("m1").unwrap().is_empty());
    }

    #[test]
    fn speakers_for_meeting_deduplicates_repeated_references() {
        let (db, _dir) = test_db();
        let alice = db.create_speaker("Alice").unwrap();

        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Persistent(alice));
        meeting.segments[1].speaker = Some(Speaker::Persistent(alice));
        db.save_meeting(&meeting).unwrap();

        let speakers = db.speakers_for_meeting("m1").unwrap();
        assert_eq!(speakers.len(), 1);
    }

    #[test]
    fn speaker_usage_counts_segments_and_meetings() {
        let (db, _dir) = test_db();
        let alice = db.create_speaker("Alice").unwrap();
        let bob = db.create_speaker("Bob").unwrap();

        let mut m1 = sample_meeting("m1");
        m1.segments[0].speaker = Some(Speaker::Persistent(alice));
        m1.segments[1].speaker = Some(Speaker::Persistent(alice));
        db.save_meeting(&m1).unwrap();
        let mut m2 = sample_meeting("m2");
        m2.segments[0].speaker = Some(Speaker::Persistent(alice));
        db.save_meeting(&m2).unwrap();

        let usage = db.speaker_usage().unwrap();
        assert_eq!(usage[&alice].segment_count, 3);
        assert_eq!(usage[&alice].meeting_count, 2);
        assert_eq!(usage[&bob], super::SpeakerUsage::default());
    }

    #[test]
    fn delete_speaker_unlabels_segments_and_removes_row() {
        let (db, _dir) = test_db();
        let alice = db.create_speaker("Alice").unwrap();
        let bob = db.create_speaker("Bob").unwrap();

        let mut meeting = sample_meeting("m1");
        meeting.segments[0].speaker = Some(Speaker::Persistent(alice));
        meeting.segments[1].speaker = Some(Speaker::Persistent(bob));
        db.save_meeting(&meeting).unwrap();

        db.delete_speaker(alice).unwrap();

        assert!(db.get_speaker(alice).unwrap().is_none());
        let loaded = db.load_meeting("m1").unwrap();
        assert_eq!(loaded.segments[0].speaker, None);
        assert_eq!(loaded.segments[1].speaker, Some(Speaker::Persistent(bob)));
    }

    #[test]
    fn delete_speaker_missing_id_errors() {
        let (db, _dir) = test_db();
        assert!(db.delete_speaker(999).is_err());
    }
}
