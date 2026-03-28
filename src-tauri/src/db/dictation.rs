use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::lock_ext::MutexExt;

use super::Database;

/// A dictation history entry
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct DictationEntry {
    pub id: String,
    pub text: String,
    pub timestamp: String,
}

impl Database {
    /// List dictation entries, newest first, with optional limit.
    pub fn list_dictation_entries(&self, limit: i64) -> Result<Vec<DictationEntry>, String> {
        let conn = self.conn.acquire()?;

        let mut stmt = conn
            .prepare("SELECT id, text, timestamp FROM dictation_entries ORDER BY timestamp DESC LIMIT ?1")
            .map_err(|e| format!("Prepare: {e}"))?;

        let entries = stmt
            .query_map(params![limit], |row| {
                Ok(DictationEntry {
                    id: row.get(0)?,
                    text: row.get(1)?,
                    timestamp: row.get(2)?,
                })
            })
            .map_err(|e| format!("Query: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Collect: {e}"))?;

        Ok(entries)
    }

    /// Add a new dictation entry.
    pub fn add_dictation_entry(&self, id: &str, text: &str, timestamp: &str) -> Result<(), String> {
        let conn = self.conn.acquire()?;

        conn.execute(
            "INSERT INTO dictation_entries (id, text, timestamp) VALUES (?1, ?2, ?3)",
            params![id, text, timestamp],
        )
        .map_err(|e| format!("Insert: {e}"))?;

        // Also index in FTS5
        conn.execute(
            "INSERT INTO text_search (content, source_type, source_id) VALUES (?1, ?2, ?3)",
            params![text, "dictation", id],
        )
        .map_err(|e| format!("FTS insert: {e}"))?;

        Ok(())
    }

    /// Delete a single dictation entry.
    pub fn delete_dictation_entry(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.acquire()?;

        conn.execute(
            "DELETE FROM text_search WHERE source_type = 'dictation' AND source_id = ?1",
            params![id],
        )
        .map_err(|e| format!("Delete FTS: {e}"))?;

        conn.execute("DELETE FROM dictation_entries WHERE id = ?1", params![id])
            .map_err(|e| format!("Delete: {e}"))?;

        Ok(())
    }

    /// Clear all dictation history.
    pub fn clear_dictation_entries(&self) -> Result<(), String> {
        let conn = self.conn.acquire()?;

        conn.execute(
            "DELETE FROM text_search WHERE source_type = 'dictation'",
            [],
        )
        .map_err(|e| format!("Delete FTS: {e}"))?;

        conn.execute("DELETE FROM dictation_entries", [])
            .map_err(|e| format!("Delete: {e}"))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::fixtures::test_db;

    #[test]
    fn add_and_list_entries() {
        let (db, _dir) = test_db();
        db.add_dictation_entry("d1", "Hello", "2024-01-01T00:00:00Z")
            .unwrap();
        db.add_dictation_entry("d2", "World", "2024-01-01T00:01:00Z")
            .unwrap();

        let entries = db.list_dictation_entries(50).unwrap();
        assert_eq!(entries.len(), 2);
        // Newest first
        assert_eq!(entries[0].text, "World");
    }

    #[test]
    fn delete_single_entry() {
        let (db, _dir) = test_db();
        db.add_dictation_entry("d1", "Hello", "2024-01-01T00:00:00Z")
            .unwrap();
        db.add_dictation_entry("d2", "World", "2024-01-01T00:01:00Z")
            .unwrap();

        db.delete_dictation_entry("d1").unwrap();
        let entries = db.list_dictation_entries(50).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "d2");
    }

    #[test]
    fn clear_all_entries() {
        let (db, _dir) = test_db();
        db.add_dictation_entry("d1", "Hello", "2024-01-01T00:00:00Z")
            .unwrap();
        db.add_dictation_entry("d2", "World", "2024-01-01T00:01:00Z")
            .unwrap();

        db.clear_dictation_entries().unwrap();
        let entries = db.list_dictation_entries(50).unwrap();
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn list_with_limit() {
        let (db, _dir) = test_db();
        for i in 0..10 {
            db.add_dictation_entry(
                &format!("d{i}"),
                &format!("Entry {i}"),
                &format!("2024-01-01T00:{i:02}:00Z"),
            )
            .unwrap();
        }

        let entries = db.list_dictation_entries(3).unwrap();
        assert_eq!(entries.len(), 3);
    }
}
