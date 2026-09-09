use rusqlite::params;
use serde::{Deserialize, Serialize};
use specta::Type;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use crate::lock_ext::MutexExt;

use super::Database;

/// Case- and accent-insensitive key a trigger is unique on. Mirrors
/// `foldSnippetTrigger` in `src/lib/features/transcription/snippets.ts` so
/// the store cannot hold two triggers the matcher would treat as one.
pub fn fold_trigger(trigger: &str) -> String {
    trigger
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase()
}

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

    pub fn add_snippet(&self, trigger: &str, expansion: &str) -> Result<SnippetEntry, String> {
        let conn = self.conn.acquire()?;
        let now = chrono::Utc::now().to_rfc3339();
        let trigger = trigger.trim();

        conn.execute(
            "INSERT INTO snippets (trigger, trigger_key, expansion, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![trigger, fold_trigger(trigger), expansion, now],
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

    pub fn update_snippet(&self, id: i64, trigger: &str, expansion: &str) -> Result<(), String> {
        let conn = self.conn.acquire()?;
        let trigger = trigger.trim();

        let updated = conn
            .execute(
                "UPDATE snippets SET trigger = ?1, trigger_key = ?2, expansion = ?3 WHERE id = ?4",
                params![trigger, fold_trigger(trigger), expansion, id],
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
    use super::fold_trigger;
    use crate::test_helpers::fixtures::test_db;

    #[test]
    fn fold_trigger_drops_case_and_accents() {
        assert_eq!(fold_trigger("Signature Mail"), "signature mail");
        assert_eq!(fold_trigger("Résumé"), "resume");
        // Decomposed input folds to the same key as precomposed input.
        assert_eq!(fold_trigger("re\u{301}sume\u{301}"), fold_trigger("résumé"));
    }

    #[test]
    fn snippets_crud() {
        let (db, _dir) = test_db();

        // Add
        let entry = db.add_snippet("brb", "be right back").unwrap();
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

    #[test]
    fn snippets_unique_trigger_folds_case_and_accents() {
        let (db, _dir) = test_db();
        db.add_snippet("café", "Café de la Paix").unwrap();
        assert!(db.add_snippet("CAFE", "another").is_err());
        assert!(db.add_snippet("Cafe\u{301}", "another").is_err());

        // Renaming onto an existing folded key is refused the same way.
        let other = db.add_snippet("bureau", "Bureau 12").unwrap();
        assert!(db.update_snippet(other.id, "Café", "x").is_err());
        let entries = db.list_snippets().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].trigger, "bureau");
        assert_eq!(entries[1].trigger, "café");
    }
}
