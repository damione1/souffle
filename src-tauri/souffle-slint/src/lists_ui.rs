//! Dictionary + Snippets sections (SOU-188 milestone 6). Both are plain CRUD
//! lists backed by dedicated sync commands (`add_dictionary_entry`,
//! `add_snippet`, ...), not a full-`AppSettings` round trip - no bounds or
//! state machine to consume here, just row formatting.

use crate::{DictionaryRow, MainWindow, SnippetRow};
use slint::{Model, VecModel};
use souffle_lib::db::snippets::SnippetEntry;
use souffle_lib::filter::DictionaryEntry;
use std::rc::Rc;

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

/// Long-lived models for the two unbounded Settings lists. They are installed
/// on the window once and then updated with row-level notifications, so a CRUD
/// operation does not replace the model or rebuild every visible delegate.
pub(crate) struct SettingsListModels {
    dictionary: Rc<VecModel<DictionaryRow>>,
    snippets: Rc<VecModel<SnippetRow>>,
}

impl SettingsListModels {
    pub(crate) fn install(window: &MainWindow) -> Rc<Self> {
        let models = Rc::new(Self {
            dictionary: Rc::new(VecModel::default()),
            snippets: Rc::new(VecModel::default()),
        });
        window.set_settings_dictionary_entries(models.dictionary.clone().into());
        window.set_settings_snippets(models.snippets.clone().into());
        models
    }

    pub(crate) fn populate_dictionary(&self, entries: &[DictionaryEntry]) {
        let rows = entries.iter().map(dictionary_row).collect();
        sync_rows_by_id(&self.dictionary, rows, |row| row.id);
    }

    pub(crate) fn populate_snippets(&self, entries: &[SnippetEntry], editing_id: Option<i64>) {
        let rows = entries
            .iter()
            .map(|entry| snippet_row(entry, editing_id))
            .collect();
        sync_rows_by_id(&self.snippets, rows, |row| row.id);
    }

    pub(crate) fn set_snippet_editing(&self, editing_id: Option<i64>) {
        for index in 0..self.snippets.row_count() {
            let Some(mut row) = self.snippets.row_data(index) else {
                continue;
            };
            let editing = Some(row.id as i64) == editing_id;
            if row.editing != editing {
                row.editing = editing;
                self.snippets.set_row_data(index, row);
            }
        }
    }
}

fn dictionary_row(entry: &DictionaryEntry) -> DictionaryRow {
    DictionaryRow {
        id: entry.id as i32,
        term: entry.term.as_str().into(),
        aliases_display: entry.pronunciation.clone().unwrap_or_default().into(),
        category: entry.category.clone().unwrap_or_default().into(),
    }
}

fn snippet_row(entry: &SnippetEntry, editing_id: Option<i64>) -> SnippetRow {
    SnippetRow {
        id: entry.id as i32,
        trigger: entry.trigger.as_str().into(),
        expansion: entry.expansion.as_str().into(),
        editing: Some(entry.id) == editing_id,
    }
}

fn sync_rows_by_id<T: Clone + PartialEq + 'static>(
    model: &VecModel<T>,
    rows: Vec<T>,
    id: impl Fn(&T) -> i32,
) {
    for (target_index, target) in rows.iter().enumerate() {
        match model.row_data(target_index) {
            Some(current) if id(&current) == id(target) => {
                if current != *target {
                    model.set_row_data(target_index, target.clone());
                }
            }
            Some(_) => {
                let existing_index = (target_index + 1..model.row_count()).find(|index| {
                    model
                        .row_data(*index)
                        .is_some_and(|current| id(&current) == id(target))
                });
                if let Some(existing_index) = existing_index {
                    model.remove(existing_index);
                }
                model.insert(target_index, target.clone());
            }
            None => model.push(target.clone()),
        }
    }
    while model.row_count() > rows.len() {
        model.remove(model.row_count() - 1);
    }
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

    #[test]
    fn sync_keeps_model_identity_and_only_changes_rows_selected_by_stable_id() {
        let model = Rc::new(VecModel::from(vec![
            (1, "one".to_string()),
            (2, "two".to_string()),
            (3, "three".to_string()),
        ]));
        let identity = Rc::clone(&model);

        sync_rows_by_id(
            &model,
            vec![
                (1, "one".to_string()),
                (2, "updated".to_string()),
                (3, "three".to_string()),
            ],
            |row| row.0,
        );

        assert!(Rc::ptr_eq(&model, &identity));
        assert_eq!(model.row_data(0), Some((1, "one".to_string())));
        assert_eq!(model.row_data(1), Some((2, "updated".to_string())));
        assert_eq!(model.row_data(2), Some((3, "three".to_string())));
    }

    #[test]
    fn sync_inserts_and_removes_by_id_without_replacing_the_model() {
        let model = Rc::new(VecModel::from(vec![(1, "one"), (3, "three")]));
        let identity = Rc::clone(&model);

        sync_rows_by_id(&model, vec![(2, "two"), (3, "three")], |row| row.0);

        assert!(Rc::ptr_eq(&model, &identity));
        assert_eq!(
            model.iter().collect::<Vec<_>>(),
            vec![(2, "two"), (3, "three")]
        );
    }
}
