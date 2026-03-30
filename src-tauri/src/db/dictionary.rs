use rusqlite::params;

use crate::filter::DictionaryEntry;
use crate::filter::soundex::soundex;
use crate::lock_ext::MutexExt;

use super::Database;

impl Database {
    pub fn list_dictionary_entries(&self) -> Result<Vec<DictionaryEntry>, String> {
        let conn = self.conn.acquire()?;
        let mut stmt = conn
            .prepare("SELECT id, term, phonetic_code, category, created_at FROM dictionary ORDER BY term COLLATE NOCASE")
            .map_err(|e| format!("Prepare list dictionary: {e}"))?;
        let entries = stmt
            .query_map([], |row| {
                Ok(DictionaryEntry {
                    id: row.get(0)?,
                    term: row.get(1)?,
                    phonetic_code: row.get(2)?,
                    category: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .map_err(|e| format!("Query dictionary: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Collect dictionary: {e}"))?;
        Ok(entries)
    }

    pub fn add_dictionary_entry(
        &self,
        term: &str,
        phonetic_code: Option<&str>,
        category: Option<&str>,
    ) -> Result<DictionaryEntry, String> {
        let conn = self.conn.acquire()?;
        let now = chrono::Utc::now().to_rfc3339();
        let effective_phonetic = phonetic_code
            .map(String::from)
            .or_else(|| soundex(term));

        conn.execute(
            "INSERT INTO dictionary (term, phonetic_code, category, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![term, effective_phonetic, category, now],
        )
        .map_err(|e| format!("Insert dictionary entry: {e}"))?;

        let id = conn.last_insert_rowid();
        Ok(DictionaryEntry {
            id,
            term: term.to_string(),
            phonetic_code: effective_phonetic,
            category: category.map(String::from),
            created_at: now,
        })
    }

    pub fn update_dictionary_entry(
        &self,
        id: i64,
        term: &str,
        phonetic_code: Option<&str>,
        category: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.acquire()?;
        let effective_phonetic = phonetic_code
            .map(String::from)
            .or_else(|| soundex(term));

        let updated = conn
            .execute(
                "UPDATE dictionary SET term = ?1, phonetic_code = ?2, category = ?3 WHERE id = ?4",
                params![term, effective_phonetic, category, id],
            )
            .map_err(|e| format!("Update dictionary entry: {e}"))?;

        if updated == 0 {
            return Err(format!("Dictionary entry {id} not found"));
        }
        Ok(())
    }

    pub fn delete_dictionary_entry(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.acquire()?;
        let deleted = conn
            .execute("DELETE FROM dictionary WHERE id = ?1", params![id])
            .map_err(|e| format!("Delete dictionary entry: {e}"))?;

        if deleted == 0 {
            return Err(format!("Dictionary entry {id} not found"));
        }
        Ok(())
    }

    pub fn clear_dictionary(&self) -> Result<(), String> {
        let conn = self.conn.acquire()?;
        conn.execute("DELETE FROM dictionary", [])
            .map_err(|e| format!("Clear dictionary: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::fixtures::test_db;

    #[test]
    fn dictionary_crud() {
        let (db, _dir) = test_db();

        // Add
        let entry = db.add_dictionary_entry("Kubernetes", None, Some("tech")).unwrap();
        assert_eq!(entry.term, "Kubernetes");
        assert!(entry.phonetic_code.is_some());
        assert_eq!(entry.category.as_deref(), Some("tech"));

        // List
        let entries = db.list_dictionary_entries().unwrap();
        assert_eq!(entries.len(), 1);

        // Update
        db.update_dictionary_entry(entry.id, "K8s", None, Some("tech")).unwrap();
        let entries = db.list_dictionary_entries().unwrap();
        assert_eq!(entries[0].term, "K8s");

        // Delete
        db.delete_dictionary_entry(entry.id).unwrap();
        let entries = db.list_dictionary_entries().unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn dictionary_clear() {
        let (db, _dir) = test_db();
        db.add_dictionary_entry("Alpha", None, None).unwrap();
        db.add_dictionary_entry("Beta", None, None).unwrap();
        assert_eq!(db.list_dictionary_entries().unwrap().len(), 2);

        db.clear_dictionary().unwrap();
        assert!(db.list_dictionary_entries().unwrap().is_empty());
    }

    #[test]
    fn dictionary_unique_term() {
        let (db, _dir) = test_db();
        db.add_dictionary_entry("Docker", None, None).unwrap();
        let result = db.add_dictionary_entry("Docker", None, None);
        assert!(result.is_err());
    }

    #[test]
    fn dictionary_auto_soundex() {
        let (db, _dir) = test_db();
        let entry = db.add_dictionary_entry("Robert", None, None).unwrap();
        assert_eq!(entry.phonetic_code.as_deref(), Some("R163"));
    }

    #[test]
    fn dictionary_explicit_phonetic_preserved() {
        let (db, _dir) = test_db();
        let entry = db.add_dictionary_entry("Souffle", Some("CUSTOM"), None).unwrap();
        assert_eq!(entry.phonetic_code.as_deref(), Some("CUSTOM"));
    }
}
