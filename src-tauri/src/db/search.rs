use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::lock_ext::MutexExt;

use super::Database;

/// Which table a full-text hit came from.
///
/// The strings are the on-disk encoding of the `text_search.source_type`
/// column, written by every version of the app, so they cannot change without
/// a migration. Declaring them here is what stops the thirteen SQL sites from
/// spelling them themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SearchSource {
    Meeting,
    Dictation,
}

impl SearchSource {
    /// Column encoding. Must stay in step with `#[serde(rename_all)]` above;
    /// `search_source_wire_encoding_matches_as_str` proves it does.
    pub const fn as_str(self) -> &'static str {
        match self {
            SearchSource::Meeting => "meeting",
            SearchSource::Dictation => "dictation",
        }
    }

    pub fn parse(raw: &str) -> Option<SearchSource> {
        match raw {
            "meeting" => Some(SearchSource::Meeting),
            "dictation" => Some(SearchSource::Dictation),
            _ => None,
        }
    }
}

impl rusqlite::ToSql for SearchSource {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(rusqlite::types::ToSqlOutput::from(self.as_str()))
    }
}

/// Search result from FTS5 full-text search
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SearchResult {
    pub source_type: SearchSource,
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
                let snippet: String = row.get(0)?;
                let source: String = row.get(1)?;
                let source_id: String = row.get(2)?;
                let rank: f64 = row.get(3)?;
                // A row whose source_type is neither known value is dropped
                // rather than surfaced: the UI already filters hits into the
                // meeting and dictation buckets and shows nothing else.
                Ok(
                    SearchSource::parse(&source).map(|source_type| SearchResult {
                        snippet,
                        source_type,
                        source_id,
                        rank,
                    }),
                )
            })
            .map_err(|e| format!("Search query: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Collect results: {e}"))?;

        Ok(results.into_iter().flatten().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::SearchSource;
    use crate::test_helpers::fixtures::{sample_meeting, test_db};

    #[test]
    fn search_source_wire_encoding_matches_as_str() {
        for source in [SearchSource::Meeting, SearchSource::Dictation] {
            let json = serde_json::to_string(&source).unwrap();
            assert_eq!(json, format!("\"{}\"", source.as_str()));
            assert_eq!(SearchSource::parse(source.as_str()), Some(source));
        }
    }

    /// AC3: the column encoding is what every past version of the app wrote.
    /// Renaming a variant must not silently reindex the user's history.
    #[test]
    fn search_source_strings_are_the_on_disk_encoding() {
        assert_eq!(SearchSource::Meeting.as_str(), "meeting");
        assert_eq!(SearchSource::Dictation.as_str(), "dictation");
    }

    #[test]
    fn indexed_rows_carry_the_legacy_source_type_text() {
        use crate::lock_ext::MutexExt;

        let (db, _dir) = test_db();
        db.save_meeting(&sample_meeting("m1")).unwrap();
        db.add_dictation_entry("d1", "Important notes", "2024-01-01T00:00:00Z")
            .unwrap();

        let conn = db.conn.acquire().unwrap();
        let mut stmt = conn
            .prepare("SELECT source_type, source_id FROM text_search ORDER BY source_id")
            .unwrap();
        let rows: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![
                ("dictation".to_string(), "d1".to_string()),
                ("meeting".to_string(), "m1".to_string()),
            ]
        );
    }

    #[test]
    fn search_finds_meeting_text() {
        let (db, _dir) = test_db();
        db.save_meeting(&sample_meeting("m1")).unwrap();

        let results = db.search_text("Hello", 20).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source_type, SearchSource::Meeting);
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
        assert_eq!(results[0].source_type, SearchSource::Dictation);
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

    #[test]
    fn search_prefers_edited_transcript_over_segments() {
        let (db, _dir) = test_db();
        db.save_meeting(&sample_meeting("m1")).unwrap();
        db.save_edited_transcript("m1", Some("Kubernetes cluster"))
            .unwrap();

        assert_eq!(db.search_text("Kubernetes", 20).unwrap().len(), 1);
        assert!(
            db.search_text("Hello", 20).unwrap().is_empty(),
            "ASR text must leave FTS after an edit"
        );

        let mut again = sample_meeting("m1");
        again.edited_transcript = Some("Kubernetes cluster".into());
        db.save_meeting(&again).unwrap();
        assert_eq!(
            db.search_text("Kubernetes", 20).unwrap().len(),
            1,
            "save_meeting must keep the edited transcript in FTS"
        );
        assert!(db.search_text("Hello", 20).unwrap().is_empty());

        db.save_edited_transcript("m1", None).unwrap();
        assert_eq!(db.search_text("Hello", 20).unwrap().len(), 1);
        assert!(db.search_text("Kubernetes", 20).unwrap().is_empty());
    }
}
