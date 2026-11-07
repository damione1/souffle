use std::sync::Arc;

use crate::db::Database;
use crate::db::dictionary::{DictionaryCorrectionSource, DictionarySuggestion};
use crate::filter::DictionaryEntry;
use crate::filter::session_terms::{cap_learned_pairs, derive_corrections_from_edit};
use crate::settings::{AppSettings, DictionaryLearningMode};
use crate::state::AppState;

/// Lists all current user dictionary entries from the database.
pub fn list_dictionary(state: Arc<AppState>) -> Result<Vec<DictionaryEntry>, String> {
    state.db.list_dictionary_entries()
}

/// Adds a new dictionary entry for text replacement.
pub fn add_dictionary_entry(
    state: Arc<AppState>,
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

/// Updates an existing dictionary entry, including its term, pronunciation, and category.
pub fn update_dictionary_entry(
    state: Arc<AppState>,
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

/// Deletes a specific dictionary entry by ID.
pub fn delete_dictionary_entry(state: Arc<AppState>, id: i64) -> Result<(), String> {
    state.db.delete_dictionary_entry(id)
}

/// Clears all entries in the user dictionary.
pub fn clear_dictionary(state: Arc<AppState>) -> Result<(), String> {
    state.db.clear_dictionary()
}

/// Apply the current persistence policy to word pairs from a post-paste edit.
pub fn learn_from_edit(
    state: Arc<AppState>,
    original: String,
    corrected: String,
) -> Result<u32, String> {
    collect_edit_corrections(
        &state.db,
        &original,
        &corrected,
        DictionaryCorrectionSource::PostPaste,
    )
}

pub fn list_dictionary_suggestions(
    state: Arc<AppState>,
) -> Result<Vec<DictionarySuggestion>, String> {
    state.db.list_dictionary_suggestions()
}

pub fn accept_dictionary_suggestion(state: Arc<AppState>, id: i64) -> Result<bool, String> {
    state.db.accept_dictionary_suggestion(id)
}

pub fn dismiss_dictionary_suggestion(state: Arc<AppState>, id: i64) -> Result<(), String> {
    state.db.dismiss_dictionary_suggestion(id)
}

pub(crate) fn collect_edit_corrections(
    db: &Database,
    original: &str,
    corrected: &str,
    source: DictionaryCorrectionSource,
) -> Result<u32, String> {
    match AppSettings::load_read_only(db)?.dictionary_learning_mode {
        DictionaryLearningMode::Disabled => Ok(0),
        DictionaryLearningMode::Suggestions => {
            let mut collected = 0;
            for correction in admissible_corrections(original, corrected) {
                collected += u32::from(db.suggest_dictionary_alias(
                    &correction.misspelling,
                    &correction.term,
                    source,
                )?);
            }
            Ok(collected)
        }
        DictionaryLearningMode::Automatic => match source {
            DictionaryCorrectionSource::PostPaste => {
                persist_learned_corrections(db, original, corrected)
            }
            DictionaryCorrectionSource::LiveMeeting => Ok(0),
        },
    }
}

fn admissible_corrections(
    original: &str,
    corrected: &str,
) -> Vec<crate::filter::session_terms::SessionCorrection> {
    cap_learned_pairs(derive_corrections_from_edit(original, corrected))
}

/// Helper to extract learned corrections from an original/corrected text pair, filter them,
/// and insert/update them in the database up to a maximum limit.
pub(crate) fn persist_learned_corrections(
    db: &Database,
    original: &str,
    corrected: &str,
) -> Result<u32, String> {
    let mut persisted = 0u32;
    for correction in admissible_corrections(original, corrected) {
        if db.upsert_dictionary_alias(&correction.term, &correction.misspelling)? {
            persisted += 1;
        }
    }

    Ok(persisted)
}

#[cfg(test)]
mod learn_from_edit {
    use super::{collect_edit_corrections, persist_learned_corrections};
    use crate::db::Database;
    use crate::db::dictionary::DictionaryCorrectionSource::{LiveMeeting, PostPaste};
    use crate::settings::{AppSettings, DictionaryLearningMode};
    use crate::test_helpers::fixtures::test_db;

    #[test]
    fn over_limit_edits_never_persist_a_partial_rewrite() {
        let original = (0..9)
            .map(|i| format!("Kubernetis{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let corrected = (0..9)
            .map(|i| format!("Kubernetes{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        for mode in DictionaryLearningMode::ALL {
            let (db, _dir) = test_db();
            AppSettings {
                dictionary_learning_mode: mode,
                ..AppSettings::default()
            }
            .save(&db)
            .unwrap();
            for source in [PostPaste, LiveMeeting] {
                assert_eq!(
                    collect_edit_corrections(&db, &original, &corrected, source).unwrap(),
                    0
                );
            }
            assert!(db.list_dictionary_entries().unwrap().is_empty());
            assert!(db.list_dictionary_suggestions().unwrap().is_empty());
        }
    }

    #[test]
    fn suggestions_deduplicate_both_sources_without_learning_and_survive_restart() {
        let (db, dir) = test_db();
        AppSettings {
            dictionary_learning_mode: DictionaryLearningMode::Suggestions,
            ..AppSettings::default()
        }
        .save(&db)
        .unwrap();
        assert_eq!(
            collect_edit_corrections(&db, "use Kubernetis", "use Kubernetes", PostPaste).unwrap(),
            1
        );
        assert_eq!(
            collect_edit_corrections(&db, "use KUBERNETIS", "use KUBERNETES", LiveMeeting).unwrap(),
            0
        );
        assert!(db.list_dictionary_entries().unwrap().is_empty());
        let pending = db.list_dictionary_suggestions().unwrap();
        assert_eq!(pending.len(), 1);
        drop(db);
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        assert_eq!(
            AppSettings::load(&db).unwrap().dictionary_learning_mode,
            DictionaryLearningMode::Suggestions
        );
        assert_eq!(db.list_dictionary_suggestions().unwrap(), pending);
        db.dismiss_dictionary_suggestion(pending[0].id).unwrap();
        drop(db);
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        assert_eq!(
            collect_edit_corrections(&db, "use Kubernetis", "use Kubernetes", PostPaste).unwrap(),
            0
        );
        assert_eq!(
            collect_edit_corrections(&db, "use KUBERNETIS", "use KUBERNETES", LiveMeeting).unwrap(),
            0
        );
        assert!(!db.accept_dictionary_suggestion(pending[0].id).unwrap());
        assert!(db.list_dictionary_suggestions().unwrap().is_empty());
        assert!(db.list_dictionary_entries().unwrap().is_empty());
    }

    #[test]
    fn exclusive_modes_preserve_post_paste_automatic_and_never_automate_meeting_edits() {
        for mode in DictionaryLearningMode::ALL {
            for source in [PostPaste, LiveMeeting] {
                let (db, _dir) = test_db();
                AppSettings {
                    dictionary_learning_mode: mode,
                    ..AppSettings::default()
                }
                .save(&db)
                .unwrap();
                collect_edit_corrections(&db, "use Kubernetis", "use Kubernetes", source).unwrap();
                let (dictionary_count, suggestion_count) = match mode {
                    DictionaryLearningMode::Disabled => (0, 0),
                    DictionaryLearningMode::Suggestions => (0, 1),
                    DictionaryLearningMode::Automatic => match source {
                        PostPaste => (1, 0),
                        LiveMeeting => (0, 0),
                    },
                };
                assert_eq!(
                    db.list_dictionary_entries().unwrap().len(),
                    dictionary_count
                );
                assert_eq!(
                    db.list_dictionary_suggestions().unwrap().len(),
                    suggestion_count
                );
                // Manual dictionary writes never consult the learning policy.
                db.add_dictionary_entry("Manual", Some("manuel"), None)
                    .unwrap();
                assert!(
                    db.list_dictionary_entries()
                        .unwrap()
                        .iter()
                        .any(|entry| entry.term == "Manual")
                );
            }
        }
    }

    #[test]
    fn suggestions_and_automatic_share_stopword_rewrite_and_short_pair_guardrails() {
        for mode in DictionaryLearningMode::ALL {
            let (db, _dir) = test_db();
            AppSettings {
                dictionary_learning_mode: mode,
                ..AppSettings::default()
            }
            .save(&db)
            .unwrap();
            for source in [PostPaste, LiveMeeting] {
                for (original, corrected) in [
                    ("on passe par Pierre", "on passe pour Pierre"),
                    ("je pense que oui", "je crois que oui"),
                    ("ok fine", "ok ok"),
                    ("same words", "same words"),
                ] {
                    assert_eq!(
                        collect_edit_corrections(&db, original, corrected, source).unwrap(),
                        0
                    );
                }
            }
            assert!(db.list_dictionary_entries().unwrap().is_empty());
            assert!(db.list_dictionary_suggestions().unwrap().is_empty());
        }
    }

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
    fn persist_learned_corrections_accepts_exactly_eight_pairs() {
        let (db, _dir) = test_db();
        let original = (0..8)
            .map(|i| format!("Kubernetis{i:02}"))
            .collect::<Vec<_>>()
            .join(" ");
        let corrected = (0..8)
            .map(|i| format!("Kubernetes{i:02}"))
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

    #[test]
    fn persist_learned_corrections_rejects_stopword_and_unrelated_pairs() {
        let (db, _dir) = test_db();
        assert_eq!(
            persist_learned_corrections(&db, "on passe par Pierre", "on passe pour Pierre")
                .unwrap(),
            0
        );
        assert_eq!(
            persist_learned_corrections(&db, "je pense que oui", "je crois que oui").unwrap(),
            0
        );
        assert!(db.list_dictionary_entries().unwrap().is_empty());
    }

    #[test]
    fn persist_learned_corrections_does_not_rewrite_function_words() {
        use crate::filter::{PipelineConfig, build_text_filters};

        let (db, _dir) = test_db();
        persist_learned_corrections(&db, "on passe par Pierre", "on passe pour Pierre").unwrap();
        let chain = build_text_filters(
            &PipelineConfig {
                vad_enabled: false,
                vad_model_path: None,
                filler_removal_enabled: false,
                stutter_collapse_enabled: false,
                dictionary_correction_enabled: true,
            },
            db.list_dictionary_entries().unwrap(),
            &[],
            &[],
        );
        assert_eq!(chain.apply("par ici"), "par ici");
        assert_eq!(chain.apply("pour la revue"), "pour la revue");
    }
}
