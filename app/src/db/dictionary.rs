use rusqlite::{Connection, OptionalExtension, params};

use crate::filter::{DictionaryEntry, pronunciation_aliases};
use crate::lock_ext::MutexExt;

use super::Database;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictionaryCorrectionSource {
    PostPaste,
    LiveMeeting,
}

impl DictionaryCorrectionSource {
    fn database_value(self) -> i64 {
        match self {
            Self::PostPaste => 0,
            Self::LiveMeeting => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictionarySuggestion {
    pub id: i64,
    pub misspelling: String,
    pub term: String,
}

fn normalized(value: &str) -> String {
    value.trim().to_lowercase()
}

#[derive(Clone, Copy)]
enum SuggestionState {
    Pending,
    Dismissed,
}

impl SuggestionState {
    fn database_value(self) -> i64 {
        match self {
            Self::Pending => 0,
            Self::Dismissed => 1,
        }
    }
}

/// The canonical learning upsert, shared by automatic learning and acceptance.
/// Its caller holds a transaction, so alias merges cannot lose concurrent edits.
fn upsert_alias(conn: &Connection, term: &str, misspelling: &str) -> Result<bool, String> {
    let entry = find_term(conn, term)?;
    if let Some((id, existing_term, pronunciation)) = entry {
        if pronunciation_aliases(&existing_term, pronunciation.as_deref())
            .iter()
            .any(|alias| normalized(alias) == normalized(misspelling))
        {
            return Ok(false);
        }
        let aliases = match pronunciation
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(raw) => format!("{raw}, {misspelling}"),
            None => misspelling.to_string(),
        };
        conn.execute(
            "UPDATE dictionary SET phonetic_code = ?1 WHERE id = ?2",
            params![aliases, id],
        )
        .map_err(|e| format!("Merge dictionary alias: {e}"))?;
    } else {
        conn.execute(
            "INSERT INTO dictionary (term, phonetic_code, created_at) VALUES (?1, ?2, ?3)",
            params![
                term.trim(),
                misspelling.trim(),
                chrono::Utc::now().to_rfc3339()
            ],
        )
        .map_err(|e| format!("Insert learned dictionary entry: {e}"))?;
    }
    Ok(true)
}

type LearnedTerm = (i64, String, Option<String>);

fn find_term(conn: &Connection, term: &str) -> Result<Option<LearnedTerm>, String> {
    let mut stmt = conn
        .prepare("SELECT id, term, phonetic_code FROM dictionary")
        .map_err(|e| format!("Find dictionary term: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|e| format!("Query dictionary term: {e}"))?;
    for row in rows {
        let entry = row.map_err(|e| format!("Read dictionary term: {e}"))?;
        if normalized(&entry.1) == normalized(term) {
            return Ok(Some(entry));
        }
    }
    Ok(None)
}

impl Database {
    pub fn upsert_dictionary_alias(&self, term: &str, misspelling: &str) -> Result<bool, String> {
        let mut conn = self.conn.acquire()?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Begin alias upsert: {e}"))?;
        let changed = upsert_alias(&tx, term, misspelling)?;
        tx.commit()
            .map_err(|e| format!("Commit alias upsert: {e}"))?;
        Ok(changed)
    }

    pub fn suggest_dictionary_alias(
        &self,
        misspelling: &str,
        term: &str,
        source: DictionaryCorrectionSource,
    ) -> Result<bool, String> {
        let conn = self.conn.acquire()?;
        if let Some((_, existing_term, pronunciation)) = find_term(&conn, term)?
            && pronunciation_aliases(&existing_term, pronunciation.as_deref())
                .iter()
                .any(|alias| normalized(alias) == normalized(misspelling))
        {
            return Ok(false);
        }
        let inserted = conn.execute(
            "INSERT INTO dictionary_suggestions (misspelling, term, misspelling_key, term_key, source) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(misspelling_key, term_key) DO NOTHING",
            params![misspelling.trim(), term.trim(), normalized(misspelling), normalized(term), source.database_value()])
            .map_err(|e| format!("Insert dictionary suggestion: {e}"))?;
        Ok(inserted != 0)
    }

    pub fn list_dictionary_suggestions(&self) -> Result<Vec<DictionarySuggestion>, String> {
        let conn = self.conn.acquire()?;
        let mut stmt = conn.prepare("SELECT id, misspelling, term FROM dictionary_suggestions WHERE state = ?1 ORDER BY id")
            .map_err(|e| format!("List dictionary suggestions: {e}"))?;
        stmt.query_map(params![SuggestionState::Pending.database_value()], |row| {
            Ok(DictionarySuggestion {
                id: row.get(0)?,
                misspelling: row.get(1)?,
                term: row.get(2)?,
            })
        })
        .map_err(|e| format!("Query dictionary suggestions: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Read dictionary suggestions: {e}"))
    }

    /// Idempotent: retrying a dismissal, or dismissing a row already accepted
    /// and removed by another request, succeeds without creating a new row.
    pub fn dismiss_dictionary_suggestion(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.acquire()?;
        conn.execute(
            "UPDATE dictionary_suggestions SET state = ?1 WHERE id = ?2",
            params![SuggestionState::Dismissed.database_value(), id],
        )
        .map_err(|e| format!("Dismiss dictionary suggestion: {e}"))?;
        Ok(())
    }

    pub fn accept_dictionary_suggestion(&self, id: i64) -> Result<bool, String> {
        let mut conn = self.conn.acquire()?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Begin suggestion acceptance: {e}"))?;
        let pair: Option<(String, String)> = tx
            .query_row(
                "SELECT misspelling, term FROM dictionary_suggestions WHERE id = ?1 AND state = ?2",
                params![id, SuggestionState::Pending.database_value()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("Read suggestion for acceptance: {e}"))?;
        let changed = match pair {
            Some((misspelling, term)) => {
                let changed = upsert_alias(&tx, &term, &misspelling)?;
                tx.execute(
                    "DELETE FROM dictionary_suggestions WHERE id = ?1",
                    params![id],
                )
                .map_err(|e| format!("Remove accepted suggestion: {e}"))?;
                changed
            }
            None => false,
        };
        tx.commit()
            .map_err(|e| format!("Commit suggestion acceptance: {e}"))?;
        Ok(changed)
    }

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
                    pronunciation: row.get(2)?,
                    category: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .map_err(|e| format!("Query dictionary: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Collect dictionary: {e}"))?;
        Ok(entries)
    }

    /// The `phonetic_code` column stores the user's pronunciation spelling
    /// (e.g. "vésix" for "V6") since schema v9; the Soundex code is derived
    /// at filter-build time, never persisted.
    pub fn add_dictionary_entry(
        &self,
        term: &str,
        pronunciation: Option<&str>,
        category: Option<&str>,
    ) -> Result<DictionaryEntry, String> {
        let conn = self.conn.acquire()?;
        let now = chrono::Utc::now().to_rfc3339();
        let pronunciation = pronunciation.map(str::trim).filter(|p| !p.is_empty());

        conn.execute(
            "INSERT INTO dictionary (term, phonetic_code, category, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![term, pronunciation, category, now],
        )
        .map_err(|e| format!("Insert dictionary entry: {e}"))?;

        let id = conn.last_insert_rowid();
        Ok(DictionaryEntry {
            id,
            term: term.to_string(),
            pronunciation: pronunciation.map(String::from),
            category: category.map(String::from),
            created_at: now,
        })
    }

    pub fn update_dictionary_entry(
        &self,
        id: i64,
        term: &str,
        pronunciation: Option<&str>,
        category: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.acquire()?;
        let pronunciation = pronunciation.map(str::trim).filter(|p| !p.is_empty());

        let updated = conn
            .execute(
                "UPDATE dictionary SET term = ?1, phonetic_code = ?2, category = ?3 WHERE id = ?4",
                params![term, pronunciation, category, id],
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
    use super::*;
    use crate::test_helpers::fixtures::test_db;

    #[test]
    fn acceptance_merges_only_missing_alias_and_preserves_known_term_metadata() {
        let (db, _dir) = test_db();
        let existing = db
            .add_dictionary_entry("Kubernetes", Some("kubes"), Some("tech"))
            .unwrap();
        db.suggest_dictionary_alias(
            "Kubernetis",
            "KUBERNETES",
            DictionaryCorrectionSource::PostPaste,
        )
        .unwrap();
        let id = db.list_dictionary_suggestions().unwrap()[0].id;
        assert!(db.accept_dictionary_suggestion(id).unwrap());
        assert!(!db.accept_dictionary_suggestion(id).unwrap());
        assert!(
            !db.suggest_dictionary_alias(
                "KUBERNETIS",
                "kubernetes",
                DictionaryCorrectionSource::LiveMeeting
            )
            .unwrap()
        );
        let entries = db.list_dictionary_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, existing.id);
        assert_eq!(entries[0].term, existing.term);
        assert_eq!(entries[0].category, existing.category);
        assert_eq!(entries[0].created_at, existing.created_at);
        assert_eq!(
            entries[0].pronunciation.as_deref(),
            Some("kubes, Kubernetis")
        );
    }

    #[test]
    fn concurrent_acceptances_are_idempotent_and_merge_aliases_without_loss() {
        let (db, _dir) = test_db();
        let db = std::sync::Arc::new(db);
        for misspelling in ["Kubernetis", "Kubernates"] {
            db.suggest_dictionary_alias(
                misspelling,
                "Kubernetes",
                DictionaryCorrectionSource::PostPaste,
            )
            .unwrap();
        }
        let pending = db.list_dictionary_suggestions().unwrap();
        let workers = (0..8)
            .map(|index| {
                let db = db.clone();
                let id = pending[index % 2].id;
                std::thread::spawn(move || db.accept_dictionary_suggestion(id).unwrap())
            })
            .collect::<Vec<_>>();
        assert_eq!(
            workers
                .into_iter()
                .map(|worker| u32::from(worker.join().unwrap()))
                .sum::<u32>(),
            2
        );
        let entries = db.list_dictionary_entries().unwrap();
        assert_eq!(entries.len(), 1);
        let aliases = pronunciation_aliases(&entries[0].term, entries[0].pronunciation.as_deref());
        assert_eq!(aliases.len(), 2);
        assert!(aliases.contains(&"Kubernetis".to_string()));
        assert!(aliases.contains(&"Kubernates".to_string()));
        assert!(db.list_dictionary_suggestions().unwrap().is_empty());
    }

    #[test]
    fn failed_acceptance_keeps_pending_pair_and_rolls_back_dictionary_write() {
        let (db, _dir) = test_db();
        db.suggest_dictionary_alias(
            "Kubernetis",
            "Kubernetes",
            DictionaryCorrectionSource::PostPaste,
        )
        .unwrap();
        let pending = db.list_dictionary_suggestions().unwrap();
        db.conn.acquire().unwrap().execute_batch("CREATE TRIGGER fail_accept BEFORE DELETE ON dictionary_suggestions BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
        assert!(db.accept_dictionary_suggestion(pending[0].id).is_err());
        assert_eq!(db.list_dictionary_suggestions().unwrap(), pending);
        assert!(db.list_dictionary_entries().unwrap().is_empty());
    }

    #[test]
    fn v17_migration_preserves_settings_dictionary_and_sidecar_compatibility() {
        let (db, dir) = test_db();
        let existing = db
            .add_dictionary_entry("Kubernetes", Some("kubes"), Some("tech"))
            .unwrap();
        db.set_setting("unrelated", "preserved").unwrap();
        db.conn
            .acquire()
            .unwrap()
            .execute_batch(
                "DROP TABLE dictionary_suggestions; UPDATE schema_version SET version = 16;",
            )
            .unwrap();
        drop(db);
        let path = dir.path().join("test.db");
        let db = Database::open(&path).unwrap();
        let entries = db.list_dictionary_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].term, existing.term);
        assert_eq!(entries[0].pronunciation, existing.pronunciation);
        assert_eq!(entries[0].category, existing.category);
        assert_eq!(entries[0].created_at, existing.created_at);
        assert_eq!(
            db.get_setting("unrelated").unwrap().as_deref(),
            Some("preserved")
        );
        assert!(db.list_dictionary_suggestions().unwrap().is_empty());
        let sidecar = souffle_mcp::db::McpDb::open(&path).unwrap();
        assert_eq!(
            sidecar.schema_version().unwrap(),
            souffle_schema::SCHEMA_VERSION
        );
    }

    #[test]
    fn dictionary_crud() {
        let (db, _dir) = test_db();

        // Add
        let entry = db
            .add_dictionary_entry("Kubernetes", None, Some("tech"))
            .unwrap();
        assert_eq!(entry.term, "Kubernetes");
        assert_eq!(entry.pronunciation, None);
        assert_eq!(entry.category.as_deref(), Some("tech"));

        // List
        let entries = db.list_dictionary_entries().unwrap();
        assert_eq!(entries.len(), 1);

        // Update
        db.update_dictionary_entry(entry.id, "K8s", None, Some("tech"))
            .unwrap();
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
    fn dictionary_stores_no_pronunciation_by_default() {
        let (db, _dir) = test_db();
        let entry = db.add_dictionary_entry("Robert", None, None).unwrap();
        assert_eq!(entry.pronunciation, None);
    }

    #[test]
    fn dictionary_pronunciation_stored_verbatim_and_blank_dropped() {
        let (db, _dir) = test_db();
        let entry = db.add_dictionary_entry("V6", Some("vésix"), None).unwrap();
        assert_eq!(entry.pronunciation.as_deref(), Some("vésix"));

        let blank = db.add_dictionary_entry("K8s", Some("   "), None).unwrap();
        assert_eq!(blank.pronunciation, None);
    }
}
