use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::Database;

/// A dictation history entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictationEntry {
    pub id: String,
    pub text: String,
    pub timestamp: String,
}

impl Database {
    /// List dictation entries, newest first, with optional limit.
    pub fn list_dictation_entries(&self, limit: i64) -> Result<Vec<DictationEntry>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock: {e}"))?;

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
        let conn = self.conn.lock().map_err(|e| format!("Lock: {e}"))?;

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
        let conn = self.conn.lock().map_err(|e| format!("Lock: {e}"))?;

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
        let conn = self.conn.lock().map_err(|e| format!("Lock: {e}"))?;

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
