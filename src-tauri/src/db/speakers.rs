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

impl Database {
    /// Create a new persistent speaker with the given display name. Returns
    /// the new row's id. `centroid`/`embedding_count` start empty; call
    /// `update_speaker_centroid` once diarization has a centroid for it.
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

    /// Overwrite a speaker's centroid, embedding count, and `last_seen_at`
    /// (typically after re-clustering embeddings into a fresh centroid).
    pub fn update_speaker_centroid(
        &self,
        id: i64,
        centroid: &[u8],
        embedding_count: i64,
        last_seen_at: DateTime<Utc>,
    ) -> Result<(), String> {
        let conn = self.conn.acquire()?;
        conn.execute(
            "UPDATE speakers SET centroid = ?1, embedding_count = ?2, last_seen_at = ?3 WHERE id = ?4",
            params![centroid, embedding_count, last_seen_at.to_rfc3339(), id],
        )
        .map_err(|e| format!("Update speaker centroid: {e}"))?;
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
    fn update_speaker_centroid_overwrites_fields() {
        let (db, _dir) = test_db();
        let id = db.create_speaker("Alice").unwrap();
        let centroid = vec![0u8; 1024]; // 256 f32s
        let now = chrono::Utc::now();

        db.update_speaker_centroid(id, &centroid, 5, now).unwrap();

        let record = db.get_speaker(id).unwrap().unwrap();
        assert_eq!(record.centroid, Some(centroid));
        assert_eq!(record.embedding_count, 5);
        // Sub-second precision may be lost in the RFC3339 round trip; compare
        // at second resolution.
        assert_eq!(record.last_seen_at.timestamp(), now.timestamp());
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
}
