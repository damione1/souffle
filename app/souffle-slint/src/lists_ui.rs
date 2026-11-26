//! Dictionary + Snippets sections (SOU-188 milestone 6). Both are plain CRUD
//! lists backed by dedicated sync commands (`add_dictionary_entry`,
//! `add_snippet`, ...), not a full-`AppSettings` round trip - no bounds or
//! state machine to consume here, just row formatting.
//!
//! The rows live in long-lived `VecModel`s whose `Rc` identity never changes
//! (`sync_rows_by_id`, from SOU-209/PR #268): a CRUD operation patches only
//! the rows whose id or content changed instead of replacing the model. The
//! lists themselves are kit `ListView`s (SOU-229), which create a row only
//! while it is on screen, so these models hold every entry.

use crate::{DictionaryRow, DictionarySuggestionRow, MainWindow, SnippetRow};
use slint::{ComponentHandle, Model, VecModel};
use souffle_lib::db::dictionary::DictionarySuggestion;
use souffle_lib::db::snippets::SnippetEntry;
use souffle_lib::filter::DictionaryEntry;
use std::cell::Cell;
use std::rc::Rc;

pub(crate) fn mutate_dictionary_suggestion(
    weak: slint::Weak<MainWindow>,
    handle: std::sync::Arc<souffle_lib::state::AppState>,
    models: Rc<SettingsListModels>,
    id: i64,
    accept: bool,
) {
    let Some(window) = weak.upgrade() else { return };
    if window.get_settings_dictionary_suggestion_busy() {
        return;
    }
    window.set_settings_dictionary_suggestion_busy(true);
    let mutation_handle = handle.clone();
    let worker = souffle_lib::async_runtime::spawn_blocking(move || {
        if accept {
            souffle_lib::commands::accept_dictionary_suggestion(mutation_handle, id).map(|_| ())
        } else {
            souffle_lib::commands::dismiss_dictionary_suggestion(mutation_handle, id)
        }
    });
    slint::spawn_local(async move {
        let outcome = match worker.await {
            Ok(result) => result,
            Err(error) => Err(error.to_string()),
        };
        let mut outcome = outcome;
        // Other dictionary actions remain usable. If one publishes while our
        // read is in flight, read again rather than overwriting its result.
        while outcome.is_ok() {
            let revision = models.dictionary_revision();
            let read_handle = handle.clone();
            let read = souffle_lib::async_runtime::spawn_blocking(move || {
                let entries = souffle_lib::commands::list_dictionary(read_handle.clone())?;
                let suggestions = souffle_lib::commands::list_dictionary_suggestions(read_handle)?;
                Ok::<_, String>((entries, suggestions))
            })
            .await;
            if weak.upgrade().is_none() {
                return;
            }
            match read {
                Ok(Ok((entries, suggestions))) => {
                    if models.populate_dictionary_snapshot(revision, &entries, &suggestions) {
                        break;
                    }
                }
                Ok(Err(error)) => outcome = Err(error),
                Err(error) => outcome = Err(error.to_string()),
            }
        }
        if let Some(window) = weak.upgrade() {
            window.set_settings_dictionary_suggestion_busy(false);
            window
                .set_settings_dictionary_suggestion_error(outcome.err().unwrap_or_default().into());
        }
    })
    .expect("dictionary suggestion future on UI thread");
}

// Row heights mirror the explicit delegate heights in
// `components/settings/dictionary_section.slint` /
// `snippets_section.slint`. Their sum sizes a list that is shorter than its
// maximum height.
const DICTIONARY_ROW_HEIGHT_PX: f32 = 51.0;
const DICTIONARY_ERROR_ROW_HEIGHT_PX: f32 = 72.0;
const SUGGESTION_ROW_HEIGHT_PX: f32 = 52.0;
const SNIPPET_ROW_HEIGHT_PX: f32 = 58.0;
const SNIPPET_ERROR_ROW_HEIGHT_PX: f32 = 80.0;
const SNIPPET_EDITING_ROW_HEIGHT_PX: f32 = 236.0;

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

/// Long-lived models of the unbounded Settings lists, installed on the
/// window once and updated with row-level notifications so a CRUD operation
/// never replaces a model.
pub(crate) struct SettingsListModels {
    window: slint::Weak<MainWindow>,
    dictionary: Rc<VecModel<DictionaryRow>>,
    snippets: Rc<VecModel<SnippetRow>>,
    suggestions: Rc<VecModel<DictionarySuggestionRow>>,
    dictionary_revision: Cell<u64>,
}

impl SettingsListModels {
    pub(crate) fn install(window: &MainWindow) -> Rc<Self> {
        let models = Rc::new(Self {
            window: window.as_weak(),
            dictionary: Rc::new(VecModel::default()),
            snippets: Rc::new(VecModel::default()),
            suggestions: Rc::new(VecModel::default()),
            dictionary_revision: Cell::new(0),
        });
        window.set_settings_dictionary_entries(models.dictionary.clone().into());
        window.set_settings_snippets(models.snippets.clone().into());
        window.set_settings_dictionary_suggestions(models.suggestions.clone().into());
        models
    }

    pub(crate) fn populate_dictionary(&self, entries: &[DictionaryEntry]) {
        self.dictionary_revision
            .set(self.dictionary_revision.get().wrapping_add(1));
        let rows = entries.iter().map(dictionary_row).collect();
        sync_rows_by_id(&self.dictionary, rows, |row| row.id);
        if let Some(window) = self.window.upgrade() {
            self.publish_dictionary_size(&window);
        }
    }

    pub(crate) fn populate_suggestions(&self, suggestions: &[DictionarySuggestion]) {
        self.dictionary_revision
            .set(self.dictionary_revision.get().wrapping_add(1));
        let rows = suggestions
            .iter()
            .map(|suggestion| DictionarySuggestionRow {
                id: suggestion.id as i32,
                misspelling: suggestion.misspelling.as_str().into(),
                term: suggestion.term.as_str().into(),
            })
            .collect();
        sync_rows_by_id(&self.suggestions, rows, |row| row.id);
        if let Some(window) = self.window.upgrade() {
            self.publish_suggestions_size(&window);
        }
    }

    pub(crate) fn dictionary_revision(&self) -> u64 {
        self.dictionary_revision.get()
    }

    /// A settings-open snapshot cannot overwrite a mutation published while
    /// its worker was still reading either of these two related lists.
    pub(crate) fn populate_dictionary_snapshot(
        &self,
        revision: u64,
        entries: &[DictionaryEntry],
        suggestions: &[DictionarySuggestion],
    ) -> bool {
        if revision == self.dictionary_revision.get() {
            self.populate_dictionary(entries);
            self.populate_suggestions(suggestions);
            true
        } else {
            false
        }
    }

    fn publish_suggestions_size(&self, window: &MainWindow) {
        let count = self.suggestions.row_count();
        window.set_settings_dictionary_suggestion_count(count as i32);
        window.set_settings_dictionary_suggestion_content_height(
            count as f32 * SUGGESTION_ROW_HEIGHT_PX,
        );
    }

    pub(crate) fn populate_snippets(&self, entries: &[SnippetEntry], editing_id: Option<i64>) {
        let rows = entries
            .iter()
            .map(|entry| snippet_row(entry, editing_id))
            .collect();
        sync_rows_by_id(&self.snippets, rows, |row| row.id);
        if let Some(window) = self.window.upgrade() {
            self.publish_snippets_size(&window);
        }
    }

    #[allow(dead_code)]
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
        if let Some(window) = self.window.upgrade() {
            self.publish_snippets_size(&window);
        }
    }

    /// Entry count and total row height of the dictionary, for its empty
    /// state and for a list shorter than its maximum height.
    fn publish_dictionary_size(&self, window: &MainWindow) {
        let error_id = window.get_settings_dictionary_delete_error_id();
        let has_error = !window.get_settings_dictionary_delete_error().is_empty();
        let height: f32 = self
            .dictionary
            .iter()
            .map(|row| {
                if has_error && row.id == error_id {
                    DICTIONARY_ERROR_ROW_HEIGHT_PX
                } else {
                    DICTIONARY_ROW_HEIGHT_PX
                }
            })
            .sum();
        window.set_settings_dictionary_count(self.dictionary.row_count() as i32);
        window.set_settings_dictionary_content_height(height);
    }

    /// Snippets counterpart of `publish_dictionary_size`.
    fn publish_snippets_size(&self, window: &MainWindow) {
        let error_id = window.get_settings_snippet_delete_error_id();
        let has_error = !window.get_settings_snippet_delete_error().is_empty();
        let height: f32 = self
            .snippets
            .iter()
            .map(|row| {
                if row.editing {
                    SNIPPET_EDITING_ROW_HEIGHT_PX
                } else if has_error && row.id == error_id {
                    SNIPPET_ERROR_ROW_HEIGHT_PX
                } else {
                    SNIPPET_ROW_HEIGHT_PX
                }
            })
            .sum();
        window.set_settings_snippets_count(self.snippets.row_count() as i32);
        window.set_settings_snippets_content_height(height);
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
    use slint::platform::{Platform, WindowAdapter, software_renderer::MinimalSoftwareWindow};

    struct TestPlatform;

    #[test]
    fn delayed_open_snapshot_cannot_restore_a_committed_suggestion() {
        let _ = slint::platform::set_platform(Box::new(TestPlatform));
        let window = MainWindow::new().unwrap();
        let models = SettingsListModels::install(&window);
        let pending = DictionarySuggestion {
            id: 1,
            misspelling: "Kubernetis".into(),
            term: "Kubernetes".into(),
        };
        models.populate_suggestions(std::slice::from_ref(&pending));
        let open_revision = models.dictionary_revision();
        let accepted = DictionaryEntry {
            id: 1,
            term: "Kubernetes".into(),
            pronunciation: Some("Kubernetis".into()),
            category: None,
            created_at: String::new(),
        };
        models.populate_dictionary(std::slice::from_ref(&accepted));
        models.populate_suggestions(&[]);
        models.populate_dictionary_snapshot(open_revision, &[], &[pending]);
        assert_eq!(models.dictionary.row_count(), 1);
        assert_eq!(
            models.dictionary.row_data(0).unwrap().term.as_str(),
            accepted.term
        );
        assert_eq!(models.suggestions.row_count(), 0);
    }

    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(Default::default()))
        }
    }

    // SOU-229: the window holds the full lists (the kit list virtualizes the
    // rows), and the published height follows the rows' states.
    #[test]
    fn the_window_receives_every_row_and_the_height_of_their_states() {
        let _ = slint::platform::set_platform(Box::new(TestPlatform));
        let window = crate::settings_kit::new_window().unwrap();
        let models = SettingsListModels::install(&window);
        let dictionary: Vec<DictionaryEntry> = (1..=500)
            .map(|id| DictionaryEntry {
                id,
                term: format!("term {id}"),
                pronunciation: None,
                category: None,
                created_at: String::new(),
            })
            .collect();
        let snippets: Vec<SnippetEntry> = (1..=500)
            .map(|id| SnippetEntry {
                id,
                trigger: format!("trigger {id}"),
                expansion: format!("expansion {id}"),
                created_at: String::new(),
            })
            .collect();
        let suggestions = (1..=500)
            .map(|id| DictionarySuggestion {
                id,
                misspelling: format!("misspelling {id}"),
                term: format!("term {id}"),
            })
            .collect::<Vec<_>>();
        models.populate_dictionary(&dictionary);
        models.populate_snippets(&snippets, None);
        models.populate_suggestions(&suggestions);

        assert_eq!(window.get_settings_dictionary_entries().row_count(), 500);
        assert_eq!(window.get_settings_snippets().row_count(), 500);
        assert_eq!(
            window.get_settings_dictionary_suggestions().row_count(),
            500
        );
        assert_eq!(window.get_settings_dictionary_count(), 500);
        assert_eq!(window.get_settings_snippets_count(), 500);
        assert_eq!(window.get_settings_dictionary_suggestion_count(), 500);
        assert_eq!(
            window.get_settings_dictionary_content_height(),
            500.0 * DICTIONARY_ROW_HEIGHT_PX
        );
        assert_eq!(
            window.get_settings_dictionary_suggestion_content_height(),
            500.0 * SUGGESTION_ROW_HEIGHT_PX
        );

        window.set_settings_dictionary_delete_error_id(7);
        window.set_settings_dictionary_delete_error("failed".into());
        models.populate_dictionary(&dictionary);
        assert_eq!(
            window.get_settings_dictionary_content_height(),
            499.0 * DICTIONARY_ROW_HEIGHT_PX + DICTIONARY_ERROR_ROW_HEIGHT_PX
        );
        models.populate_snippets(&snippets, Some(3));
        assert_eq!(
            window.get_settings_snippets_content_height(),
            499.0 * SNIPPET_ROW_HEIGHT_PX + SNIPPET_EDITING_ROW_HEIGHT_PX
        );
    }

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
