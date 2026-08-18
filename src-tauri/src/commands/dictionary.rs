use tauri::State;

use crate::db::Database;
use crate::filter::session_terms::derive_corrections_from_edit;
use crate::filter::{pronunciation_aliases, DictionaryEntry};
use crate::state::AppState;

const MAX_LEARNED_PAIRS: usize = 8;
const MIN_TOKEN_LEN: usize = 3;

#[tauri::command]
#[specta::specta]
pub fn list_dictionary(state: State<'_, AppState>) -> Result<Vec<DictionaryEntry>, String> {
    state.db.list_dictionary_entries()
}

#[tauri::command]
#[specta::specta]
pub fn add_dictionary_entry(
    state: State<'_, AppState>,
    term: String,
    pronunciation: Option<String>,
    category: Option<String>,
) -> Result<DictionaryEntry, String> {
    let term = term.trim();
    if term.is_empty() {
        return Err("Term cannot be empty".into());
    }
    state
        .db
        .add_dictionary_entry(term, pronunciation.as_deref(), category.as_deref())
}

#[tauri::command]
#[specta::specta]
pub fn update_dictionary_entry(
    state: State<'_, AppState>,
    id: i64,
    term: String,
    pronunciation: Option<String>,
    category: Option<String>,
) -> Result<(), String> {
    let term = term.trim();
    if term.is_empty() {
        return Err("Term cannot be empty".into());
    }
    state
        .db
        .update_dictionary_entry(id, term, pronunciation.as_deref(), category.as_deref())
}

#[tauri::command]
#[specta::specta]
pub fn delete_dictionary_entry(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state.db.delete_dictionary_entry(id)
}

#[tauri::command]
#[specta::specta]
pub fn clear_dictionary(state: State<'_, AppState>) -> Result<(), String> {
    state.db.clear_dictionary()
}

/// Persist word-level misspelling→term pairs from a post-paste edit.
#[tauri::command]
#[specta::specta]
pub fn learn_from_edit(
    state: State<'_, AppState>,
    original: String,
    corrected: String,
) -> Result<u32, String> {
    persist_learned_corrections(&state.db, &original, &corrected)
}

pub(crate) fn persist_learned_corrections(
    db: &Database,
    original: &str,
    corrected: &str,
) -> Result<u32, String> {
    let corrections: Vec<_> = derive_corrections_from_edit(original, corrected)
        .into_iter()
        .filter(|correction| is_persistable_pair(&correction.misspelling, &correction.term))
        .take(MAX_LEARNED_PAIRS)
        .collect();

    let mut entries = db.list_dictionary_entries()?;
    let mut persisted = 0u32;

    for correction in corrections {
        if upsert_learned_alias(&mut entries, db, &correction.term, &correction.misspelling)? {
            persisted += 1;
        }
    }

    Ok(persisted)
}

fn is_persistable_pair(misspelling: &str, term: &str) -> bool {
    misspelling.chars().count() >= MIN_TOKEN_LEN
        && term.chars().count() >= MIN_TOKEN_LEN
        && !misspelling.eq_ignore_ascii_case(term)
}

fn find_entry_by_term<'a>(
    entries: &'a [DictionaryEntry],
    term: &str,
) -> Option<&'a DictionaryEntry> {
    entries
        .iter()
        .find(|entry| entry.term.eq_ignore_ascii_case(term))
}

fn append_misspelling_alias(entry: &DictionaryEntry, misspelling: &str) -> Option<String> {
    let existing = pronunciation_aliases(&entry.term, entry.pronunciation.as_deref());
    if existing
        .iter()
        .any(|alias| alias.eq_ignore_ascii_case(misspelling))
    {
        return None;
    }
    match entry
        .pronunciation
        .as_deref()
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
    {
        Some(raw) => Some(format!("{raw}, {misspelling}")),
        None => Some(misspelling.to_string()),
    }
}

fn is_unique_constraint(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("unique") || lower.contains("constraint failed")
}

fn upsert_learned_alias(
    entries: &mut Vec<DictionaryEntry>,
    db: &Database,
    term: &str,
    misspelling: &str,
) -> Result<bool, String> {
    if let Some(existing) = find_entry_by_term(entries, term).cloned() {
        return apply_alias_update(entries, db, &existing, misspelling);
    }

    match db.add_dictionary_entry(term, Some(misspelling), None) {
        Ok(entry) => {
            entries.push(entry);
            Ok(true)
        }
        Err(err) if is_unique_constraint(&err) => {
            *entries = db.list_dictionary_entries()?;
            match find_entry_by_term(entries, term).cloned() {
                Some(existing) => apply_alias_update(entries, db, &existing, misspelling),
                None => Err(err),
            }
        }
        Err(err) => Err(err),
    }
}

fn apply_alias_update(
    entries: &mut [DictionaryEntry],
    db: &Database,
    existing: &DictionaryEntry,
    misspelling: &str,
) -> Result<bool, String> {
    let Some(pronunciation) = append_misspelling_alias(existing, misspelling) else {
        return Ok(false);
    };
    db.update_dictionary_entry(
        existing.id,
        &existing.term,
        Some(&pronunciation),
        existing.category.as_deref(),
    )?;
    if let Some(row) = entries.iter_mut().find(|entry| entry.id == existing.id) {
        row.pronunciation = Some(pronunciation);
    }
    Ok(true)
}

#[cfg(test)]
mod learn_from_edit {
    use super::persist_learned_corrections;
    use crate::test_helpers::fixtures::test_db;

    #[test]
    fn persist_learned_corrections_inserts_new_term() {
        let (db, _dir) = test_db();
        let count = persist_learned_corrections(
            &db,
            "We use Kubernetis for deploys",
            "We use Kubernetes for deploys",
        )
        .unwrap();
        assert_eq!(count, 1);

        let entries = db.list_dictionary_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].term, "Kubernetes");
        assert_eq!(entries[0].pronunciation.as_deref(), Some("Kubernetis"));
    }

    #[test]
    fn persist_learned_corrections_appends_alias_to_existing_term() {
        let (db, _dir) = test_db();
        db.add_dictionary_entry("Kubernetes", Some("kubes"), Some("tech"))
            .unwrap();

        let count = persist_learned_corrections(
            &db,
            "We use Kubernetis for deploys",
            "We use Kubernetes for deploys",
        )
        .unwrap();
        assert_eq!(count, 1);

        let entries = db.list_dictionary_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].term, "Kubernetes");
        assert_eq!(
            entries[0].pronunciation.as_deref(),
            Some("kubes, Kubernetis")
        );
        assert_eq!(entries[0].category.as_deref(), Some("tech"));
    }

    #[test]
    fn persist_learned_corrections_skips_duplicate_alias() {
        let (db, _dir) = test_db();
        db.add_dictionary_entry("Kubernetes", Some("Kubernetis"), None)
            .unwrap();

        let count = persist_learned_corrections(
            &db,
            "We use Kubernetis for deploys",
            "We use Kubernetes for deploys",
        )
        .unwrap();
        assert_eq!(count, 0);
        assert_eq!(
            db.list_dictionary_entries().unwrap()[0]
                .pronunciation
                .as_deref(),
            Some("Kubernetis")
        );
    }

    #[test]
    fn persist_learned_corrections_unique_race_falls_back_to_update() {
        let (db, _dir) = test_db();
        db.add_dictionary_entry("Kubernetes", None, None).unwrap();

        let count = persist_learned_corrections(
            &db,
            "We use Kubernetis for deploys",
            "We use Kubernetes for deploys",
        )
        .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            db.list_dictionary_entries().unwrap()[0]
                .pronunciation
                .as_deref(),
            Some("Kubernetis")
        );
    }

    #[test]
    fn persist_learned_corrections_caps_at_eight_pairs() {
        let (db, _dir) = test_db();
        let original = (0..11)
            .map(|i| format!("mis{i:02}x"))
            .collect::<Vec<_>>()
            .join(" ");
        let corrected = (0..11)
            .map(|i| format!("term{i:02}x"))
            .collect::<Vec<_>>()
            .join(" ");

        let count = persist_learned_corrections(&db, &original, &corrected).unwrap();
        assert_eq!(count, 8);
        assert_eq!(db.list_dictionary_entries().unwrap().len(), 8);
    }

    #[test]
    fn learn_from_edit_skips_identical_and_short_tokens() {
        let (db, _dir) = test_db();
        assert_eq!(
            persist_learned_corrections(&db, "hello world", "hello world").unwrap(),
            0
        );
        assert_eq!(
            persist_learned_corrections(&db, "ok fine", "ok ok").unwrap(),
            0
        );
        assert!(db.list_dictionary_entries().unwrap().is_empty());
    }

    #[test]
    fn persist_learned_corrections_appends_second_misspelling_in_batch() {
        let (db, _dir) = test_db();
        let count = persist_learned_corrections(
            &db,
            "Kubernetis and Kubernates today",
            "Kubernetes and Kubernetes today",
        )
        .unwrap();
        assert_eq!(count, 2);

        let entries = db.list_dictionary_entries().unwrap();
        assert_eq!(entries.len(), 1);
        let pronunciation = entries[0].pronunciation.as_deref().unwrap();
        assert!(pronunciation.contains("Kubernetis"));
        assert!(pronunciation.contains("Kubernates"));
    }
}
