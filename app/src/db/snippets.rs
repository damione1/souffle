use rusqlite::params;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use crate::lock_ext::MutexExt;

use super::Database;

/// Case- and accent-insensitive key a trigger is unique on. The same fold is
/// what [`apply_snippet`] compares with, so the store cannot hold two
/// triggers the matcher would treat as one.
pub fn fold_trigger(trigger: &str) -> String {
    trigger
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase()
}

/// Byte offset in `text` just past the prefix whose folded form is
/// `folded_len` bytes long. Folding is done per char so the raw and folded
/// strings stay aligned even when the input is decomposed (NFD), where a
/// `trigger.len()` slice would land mid-character. Combining marks that
/// trail the last base character belong to the prefix too.
fn raw_prefix_end(text: &str, folded_len: usize) -> usize {
    let mut folded = 0;
    for (offset, ch) in text.char_indices() {
        if folded >= folded_len && !is_combining_mark(ch) {
            return offset;
        }
        let mut buf = [0u8; 4];
        folded += fold_trigger(ch.encode_utf8(&mut buf)).len();
    }
    text.len()
}

/// Punctuation the engine glues to the trigger ("signature mail, et …"):
/// kept attached to the expansion rather than separated by a space.
const LEADING_PUNCTUATION: [char; 6] = ['.', ',', '!', '?', ':', ';'];

/// Replace a spoken trigger at the start of `text` by its expansion.
///
/// Longest trigger wins, the trigger must end on a word boundary, and the
/// rest of the transcript is kept after the expansion. Returns `None` when
/// no snippet matches so the caller falls through to the polish path.
pub fn apply_snippet(text: &str, snippets: &[SnippetEntry]) -> Option<String> {
    if snippets.is_empty() {
        return None;
    }

    let folded = fold_trigger(text);
    let mut candidates: Vec<(&SnippetEntry, String)> = snippets
        .iter()
        .map(|snippet| (snippet, fold_trigger(snippet.trigger.trim())))
        .filter(|(_, key)| !key.is_empty())
        .collect();
    // Stable, so equal-length keys keep the list order.
    candidates.sort_by_key(|(_, key)| std::cmp::Reverse(key.chars().count()));

    for (snippet, key) in candidates {
        let Some(after) = folded.strip_prefix(key.as_str()) else {
            continue;
        };
        if after.chars().next().is_some_and(char::is_alphanumeric) {
            continue;
        }

        let remainder = text[raw_prefix_end(text, key.len())..].trim_start();
        if remainder.is_empty() {
            return Some(snippet.expansion.clone());
        }
        if remainder.starts_with(LEADING_PUNCTUATION) {
            return Some(format!("{}{remainder}", snippet.expansion));
        }
        return Some(format!("{} {remainder}", snippet.expansion));
    }
    None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    use super::{SnippetEntry, apply_snippet, fold_trigger};
    use crate::test_helpers::fixtures::test_db;

    fn snippet(id: i64, trigger: &str, expansion: &str) -> SnippetEntry {
        SnippetEntry {
            id,
            trigger: trigger.to_string(),
            expansion: expansion.to_string(),
            created_at: String::new(),
        }
    }

    fn signature() -> SnippetEntry {
        snippet(1, "signature mail", "Cordialement,\nDamien")
    }

    #[test]
    fn apply_snippet_returns_none_without_a_match() {
        assert_eq!(apply_snippet("Signature mail", &[]), None);
        assert_eq!(apply_snippet("Bonjour à tous", &[signature()]), None);
        // A trigger only counts at the start of the transcript.
        assert_eq!(
            apply_snippet("Ma signature mail est prête", &[signature()]),
            None
        );
    }

    #[test]
    fn apply_snippet_keeps_the_rest_of_the_transcript() {
        assert_eq!(
            apply_snippet("Signature mail, et à bientôt.", &[signature()]).as_deref(),
            Some("Cordialement,\nDamien, et à bientôt.")
        );
        assert_eq!(
            apply_snippet("Signature mail et à bientôt", &[signature()]).as_deref(),
            Some("Cordialement,\nDamien et à bientôt")
        );
        assert_eq!(
            apply_snippet("Signature mail", &[signature()]).as_deref(),
            Some("Cordialement,\nDamien")
        );
    }

    #[test]
    fn apply_snippet_requires_a_word_boundary_after_the_trigger() {
        let brb = [snippet(2, "brb", "be right back")];
        assert_eq!(apply_snippet("brbx", &brb), None);
        assert_eq!(apply_snippet("brb2", &brb), None);
        assert_eq!(
            apply_snippet("brb, see you", &brb).as_deref(),
            Some("be right back, see you")
        );
        assert_eq!(
            apply_snippet("brb.", &brb).as_deref(),
            Some("be right back.")
        );
    }

    #[test]
    fn apply_snippet_prefers_the_longest_trigger_regardless_of_order() {
        let short = snippet(1, "signature", "SHORT");
        let long = snippet(2, "signature mail", "LONG");
        let forward = [short.clone(), long.clone()];
        let backward = [long, short];
        assert_eq!(
            apply_snippet("Signature mail pro", &forward).as_deref(),
            Some("LONG pro")
        );
        assert_eq!(
            apply_snippet("Signature mail pro", &backward).as_deref(),
            Some("LONG pro")
        );
        assert_eq!(
            apply_snippet("Signature pro", &backward).as_deref(),
            Some("SHORT pro")
        );
    }

    #[test]
    fn apply_snippet_matches_case_and_accent_insensitively() {
        let resume = [snippet(1, "resume", "Résumé de la réunion :")];
        let cafe = [snippet(2, "Café", "Café de la Paix")];
        assert_eq!(
            apply_snippet("Résumé bonjour", &resume).as_deref(),
            Some("Résumé de la réunion : bonjour")
        );
        assert_eq!(
            apply_snippet("RÉSUMÉ", &resume).as_deref(),
            Some("Résumé de la réunion :")
        );
        assert_eq!(
            apply_snippet("cafe demain", &cafe).as_deref(),
            Some("Café de la Paix demain")
        );
    }

    #[test]
    fn apply_snippet_cuts_the_remainder_at_the_matched_prefix_on_decomposed_input() {
        // "résumé bonjour" in NFD: a `trigger.len()` slice would leave the
        // last accent in the remainder.
        let resume = [snippet(1, "resume", "R")];
        assert_eq!(
            apply_snippet("re\u{301}sume\u{301} bonjour", &resume).as_deref(),
            Some("R bonjour")
        );
        // Mirror case: a decomposed trigger against precomposed text.
        let decomposed_trigger = [snippet(2, "re\u{301}sume\u{301}", "R")];
        assert_eq!(
            apply_snippet("Résumé bonjour", &decomposed_trigger).as_deref(),
            Some("R bonjour")
        );
    }

    #[test]
    fn apply_snippet_ignores_blank_triggers() {
        assert_eq!(apply_snippet("hello", &[snippet(1, "   ", "X")]), None);
    }

    #[test]
    fn fold_trigger_drops_case_and_accents() {
        assert_eq!(fold_trigger("Signature Mail"), "signature mail");
        assert_eq!(fold_trigger("Résumé"), "resume");
        // Decomposed input folds to the same key as precomposed input.
        assert_eq!(fold_trigger("re\u{301}sume\u{301}"), fold_trigger("résumé"));
    }

    #[derive(serde::Deserialize)]
    struct FoldTestCase {
        input: String,
        expected: String,
    }

    #[test]
    fn fold_trigger_matches_fixtures() {
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/snippet-fold-test-cases.json");
        let file = std::fs::File::open(fixture_path).expect("failed to open fixture");
        let test_cases: Vec<FoldTestCase> =
            serde_json::from_reader(file).expect("failed to parse fixture");

        for case in test_cases {
            assert_eq!(
                fold_trigger(&case.input),
                case.expected,
                "Failed on input: {}",
                case.input
            );
        }
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
