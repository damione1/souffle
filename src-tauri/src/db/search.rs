use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::lock_ext::MutexExt;

use super::Database;

/// Search result from FTS5 full-text search
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SearchResult {
    pub source_type: String,
    pub source_id: String,
    pub snippet: String,
    pub rank: f64,
}

impl Database {
    /// Full-text search across meetings and dictation entries.
    /// Returns highlighted snippets with `<mark>` tags around matched terms.
    pub fn search_text(&self, query: &str, limit: i64) -> Result<Vec<SearchResult>, String> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }

        let conn = self.conn.acquire()?;

        let mut stmt = conn
            .prepare(
                "SELECT snippet(text_search, 0, '<mark>', '</mark>', '...', 32),
                        source_type,
                        source_id,
                        rank
                 FROM text_search
                 WHERE text_search MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )
            .map_err(|e| format!("Prepare search: {e}"))?;

        let results = stmt
            .query_map(params![query, limit], |row| {
                Ok(SearchResult {
                    snippet: row.get(0)?,
                    source_type: row.get(1)?,
                    source_id: row.get(2)?,
                    rank: row.get(3)?,
                })
            })
            .map_err(|e| format!("Search query: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Collect results: {e}"))?;

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::fixtures::{sample_meeting, test_db};

    #[test]
    fn search_finds_meeting_text() {
        let (db, _dir) = test_db();
        db.save_meeting(&sample_meeting("m1")).unwrap();

        let results = db.search_text("Hello", 20).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source_type, "meeting");
        assert_eq!(results[0].source_id, "m1");
        assert!(results[0].snippet.contains("<mark>"));
    }

    #[test]
    fn search_finds_dictation_text() {
        let (db, _dir) = test_db();
        db.add_dictation_entry("d1", "Important meeting notes", "2024-01-01T00:00:00Z")
            .unwrap();

        let results = db.search_text("Important", 20).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source_type, "dictation");
    }

    #[test]
    fn search_empty_query_returns_empty() {
        let (db, _dir) = test_db();
        let results = db.search_text("", 20).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_no_match_returns_empty() {
        let (db, _dir) = test_db();
        db.save_meeting(&sample_meeting("m1")).unwrap();
        let results = db.search_text("nonexistent_xyz", 20).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_respects_limit() {
        let (db, _dir) = test_db();
        for i in 0..5 {
            db.add_dictation_entry(
                &format!("d{i}"),
                &format!("Hello world entry {i}"),
                &format!("2024-01-01T00:{i:02}:00Z"),
            )
            .unwrap();
        }

        let results = db.search_text("Hello", 3).unwrap();
        assert_eq!(results.len(), 3);
    }
}
