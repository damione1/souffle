//! Dictionary + Snippets sections (SOU-188 milestone 6). Both are plain CRUD
//! lists backed by dedicated sync commands (`add_dictionary_entry`,
//! `add_snippet`, ...), not a full-`AppSettings` round trip - no bounds or
//! state machine to consume here, just row formatting.

use crate::{DictionaryRow, MainWindow, SnippetRow};
use souffle_lib::db::snippets::SnippetEntry;
use souffle_lib::filter::DictionaryEntry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DictionaryAddDraft {
    pub term: String,
    pub pronunciation: String,
    pub category: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnippetDraft {
    pub trigger: String,
    pub expansion: String,
}

#[derive(Debug)]
pub(crate) enum ListMutation<T, D> {
    Committed(T),
    Rejected { draft: D, error: String },
}

pub(crate) fn submit_dictionary_add(
    draft: DictionaryAddDraft,
    add: impl FnOnce(String, Option<String>, Option<String>) -> Result<DictionaryEntry, String>,
) -> ListMutation<DictionaryEntry, DictionaryAddDraft> {
    let pronunciation = (!draft.pronunciation.is_empty()).then(|| draft.pronunciation.clone());
    let category = (!draft.category.is_empty()).then(|| draft.category.clone());
    match add(draft.term.clone(), pronunciation, category) {
        Ok(entry) => ListMutation::Committed(entry),
        Err(error) => ListMutation::Rejected { draft, error },
    }
}

pub(crate) fn submit_dictionary_delete(
    id: i64,
    delete: impl FnOnce(i64) -> Result<(), String>,
) -> ListMutation<(), i64> {
    match delete(id) {
        Ok(()) => ListMutation::Committed(()),
        Err(error) => ListMutation::Rejected { draft: id, error },
    }
}

pub(crate) fn submit_snippet_add(
    draft: SnippetDraft,
    add: impl FnOnce(&str, &str) -> Result<SnippetEntry, String>,
) -> ListMutation<SnippetEntry, SnippetDraft> {
    match add(&draft.trigger, &draft.expansion) {
        Ok(entry) => ListMutation::Committed(entry),
        Err(error) => ListMutation::Rejected { draft, error },
    }
}

pub(crate) fn submit_snippet_delete(
    id: i64,
    delete: impl FnOnce(i64) -> Result<(), String>,
) -> ListMutation<(), i64> {
    match delete(id) {
        Ok(()) => ListMutation::Committed(()),
        Err(error) => ListMutation::Rejected { draft: id, error },
    }
}

pub(crate) fn submit_snippet_update(
    draft: SnippetDraft,
    update: impl FnOnce(&str, &str) -> Result<(), String>,
) -> ListMutation<(), SnippetDraft> {
    match update(&draft.trigger, &draft.expansion) {
        Ok(()) => ListMutation::Committed(()),
        Err(error) => ListMutation::Rejected { draft, error },
    }
}

pub fn populate_dictionary(window: &MainWindow, entries: &[DictionaryEntry]) {
    let rows: Vec<DictionaryRow> = entries
        .iter()
        .map(|entry| DictionaryRow {
            id: entry.id as i32,
            term: entry.term.as_str().into(),
            aliases_display: entry.pronunciation.clone().unwrap_or_default().into(),
            category: entry.category.clone().unwrap_or_default().into(),
        })
        .collect();
    window.set_settings_dictionary_entries(std::rc::Rc::new(slint::VecModel::from(rows)).into());
}

pub fn populate_snippets(window: &MainWindow, entries: &[SnippetEntry], editing_id: Option<i64>) {
    let rows: Vec<SnippetRow> = entries
        .iter()
        .map(|entry| SnippetRow {
            id: entry.id as i32,
            trigger: entry.trigger.as_str().into(),
            expansion: entry.expansion.as_str().into(),
            editing: Some(entry.id) == editing_id,
        })
        .collect();
    window.set_settings_snippets(std::rc::Rc::new(slint::VecModel::from(rows)).into());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_dictionary_database_failures_return_every_field_and_deleted_id() {
        let draft = DictionaryAddDraft {
            term: "Kubernetes".into(),
            pronunciation: "k8s".into(),
            category: "tech".into(),
        };
        match submit_dictionary_add(draft.clone(), |_, _, _| {
            Err("injected dictionary insert failure".into())
        }) {
            ListMutation::Rejected {
                draft: retained,
                error,
            } => {
                assert_eq!(retained, draft);
                assert!(error.contains("injected"));
            }
            ListMutation::Committed(_) => panic!("injected insert failure committed"),
        }

        match submit_dictionary_delete(7, |_| Err("injected dictionary delete failure".into())) {
            ListMutation::Rejected { draft: id, error } => {
                assert_eq!(id, 7);
                assert!(error.contains("delete"));
            }
            ListMutation::Committed(()) => panic!("injected delete failure committed"),
        }
    }

    #[test]
    fn injected_snippet_database_failures_return_add_edit_and_delete_drafts() {
        let add_draft = SnippetDraft {
            trigger: "adresse".into(),
            expansion: "123 rue Test".into(),
        };
        match submit_snippet_add(add_draft.clone(), |_, _| {
            Err("injected snippet insert failure".into())
        }) {
            ListMutation::Rejected { draft, error } => {
                assert_eq!(draft, add_draft);
                assert!(error.contains("insert"));
            }
            ListMutation::Committed(_) => panic!("injected insert failure committed"),
        }

        let edit_draft = SnippetDraft {
            trigger: "signature pro".into(),
            expansion: "Bien cordialement".into(),
        };
        match submit_snippet_update(edit_draft.clone(), |_, _| {
            Err("injected snippet update failure".into())
        }) {
            ListMutation::Rejected { draft, error } => {
                assert_eq!(draft, edit_draft);
                assert!(error.contains("update"));
            }
            ListMutation::Committed(()) => panic!("injected update failure committed"),
        }

        match submit_snippet_delete(9, |_| Err("injected snippet delete failure".into())) {
            ListMutation::Rejected { draft: id, error } => {
                assert_eq!(id, 9);
                assert!(error.contains("delete"));
            }
            ListMutation::Committed(()) => panic!("injected delete failure committed"),
        }
    }
}
