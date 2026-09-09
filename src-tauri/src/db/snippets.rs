use rusqlite::params;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::lock_ext::MutexExt;

use super::Database;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SnippetEntry {
    pub id: i64,
    pub trigger: String,
    pub expansion: String,
    pub created_at: String,
}

impl Database {
    pub fn list_snippets(&self) -> Result<Vec<SnippetEntry>, String> {
        let conn = self.conn.acquire()?;
        let mut stmt = conn
            .prepare("SELECT id, trigger, expansion, created_at FROM snippets ORDER BY trigger COLLATE NOCASE")
            .map_err(|e| format!("Prepare list snippets: {e}"))?;
        let entries = stmt
            .query_map([], |row| {
                Ok(SnippetEntry {
                    id: row.get(0)?,
                    trigger: row.get(1)?,
                    expansion: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .map_err(|e| format!("Query snippets: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Collect snippets: {e}"))?;
        Ok(entries)
    }

    pub fn add_snippet(
        &self,
        trigger: &str,
        expansion: &str,
    ) -> Result<SnippetEntry, String> {
        let conn = self.conn.acquire()?;
        let now = chrono::Utc::now().to_rfc3339();
        let trigger = trigger.trim();

        conn.execute(
            "INSERT INTO snippets (trigger, expansion, created_at) VALUES (?1, ?2, ?3)",
            params![trigger, expansion, now],
        )
        .map_err(|e| format!("Insert snippet entry: {e}"))?;

        let id = conn.last_insert_rowid();
        Ok(SnippetEntry {
            id,
            trigger: trigger.to_string(),
            expansion: expansion.to_string(),
            created_at: now,
        })
    }

    pub fn update_snippet(
        &self,
        id: i64,
        trigger: &str,
        expansion: &str,
    ) -> Result<(), String> {
        let conn = self.conn.acquire()?;
        let trigger = trigger.trim();

        let updated = conn
            .execute(
                "UPDATE snippets SET trigger = ?1, expansion = ?2 WHERE id = ?3",
                params![trigger, expansion, id],
            )
            .map_err(|e| format!("Update snippet entry: {e}"))?;

        if updated == 0 {
            return Err(format!("Snippet entry {id} not found"));
        }
        Ok(())
    }

    pub fn delete_snippet(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.acquire()?;
        let deleted = conn
            .execute("DELETE FROM snippets WHERE id = ?1", params![id])
            .map_err(|e| format!("Delete snippet entry: {e}"))?;

        if deleted == 0 {
            return Err(format!("Snippet entry {id} not found"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::fixtures::test_db;

    #[test]
    fn snippets_crud() {
        let (db, _dir) = test_db();

        // Add
        let entry = db
            .add_snippet("brb", "be right back")
            .unwrap();
        assert_eq!(entry.trigger, "brb");
        assert_eq!(entry.expansion, "be right back");

        // List
        let entries = db.list_snippets().unwrap();
        assert_eq!(entries.len(), 1);

        // Update
        db.update_snippet(entry.id, "brb", "be right back!")
            .unwrap();
        let entries = db.list_snippets().unwrap();
        assert_eq!(entries[0].expansion, "be right back!");

        // Delete
        db.delete_snippet(entry.id).unwrap();
        let entries = db.list_snippets().unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn snippets_unique_trigger() {
        let (db, _dir) = test_db();
        db.add_snippet("omg", "oh my god").unwrap();
        let result = db.add_snippet("omg", "oh my gosh");
        assert!(result.is_err());
    }
}
