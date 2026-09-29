//! Dictionary + Snippets sections (SOU-188 milestone 6). Both are plain CRUD
//! lists backed by dedicated sync commands (`add_dictionary_entry`,
//! `add_snippet`, ...), not a full-`AppSettings` round trip - no bounds or
//! state machine to consume here, just row formatting.
//!
//! Two distinct mechanisms live here (SOU-229 splits them apart honestly):
//!
//! - **Incremental sync by id** (`sync_rows_by_id`, from SOU-209/PR #268):
//!   the full row lists live in long-lived `VecModel`s whose `Rc` identity
//!   never changes; a CRUD operation patches only the rows whose id/content
//!   actually changed instead of rebuilding the whole model. This is *not*
//!   virtualization - it bounds notifications, not instantiation.
//! - **Render windowing** (SOU-229): Slint's `ListView` is a plain
//!   `ScrollView { @children }` (`i-slint-compiler-1.17.1/widgets/common/
//!   listview.slint`) that mounts one delegate per model row no matter what
//!   is on screen. So, same as the transcript list (SOU-187 AC15, see
//!   `transcript::visible_window`), the window only ever receives a *slice*
//!   model of the on-screen rows plus a margin, with Rust-computed
//!   spacer-before/spacer-after paddings standing in for everything else.

use crate::transcript::visible_window;
use crate::{DictionaryRow, MainWindow, SnippetRow};
use slint::{ComponentHandle, Model, VecModel};
use souffle_lib::db::snippets::SnippetEntry;
use souffle_lib::filter::DictionaryEntry;
use std::cell::Cell;
use std::rc::Rc;

// Row heights mirror the explicit delegate heights in
// `components/settings/dictionary_section.slint` /
// `snippets_section.slint`. Like the transcript's estimates, a stale error/
// editing state only skews spacer sizing a little - mounted rows always lay
// out their real height.
const DICTIONARY_ROW_HEIGHT_PX: f32 = 64.0;
const DICTIONARY_ERROR_ROW_HEIGHT_PX: f32 = 88.0;
const DICTIONARY_VIEWPORT_HEIGHT_PX: f32 = 360.0;
const SNIPPET_ROW_HEIGHT_PX: f32 = 96.0;
const SNIPPET_ERROR_ROW_HEIGHT_PX: f32 = 116.0;
const SNIPPET_EDITING_ROW_HEIGHT_PX: f32 = 236.0;
const SNIPPETS_VIEWPORT_HEIGHT_PX: f32 = 420.0;
// Two viewport-heights of pre-mounted margin on each side, so ordinary
// scrolling hits already-mounted rows and the re-window is invisible.
const SETTINGS_LIST_SCROLL_MARGIN_FACTOR: f32 = 2.0;

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

/// Long-lived models for the two unbounded Settings lists. `dictionary` /
/// `snippets` hold the *full* row lists (source of truth, updated with
/// row-level notifications so a CRUD operation never replaces the model),
/// while `dictionary_view` / `snippets_view` are the bounded slice models
/// actually installed on the window - only the on-screen rows plus a margin
/// are ever mounted as Slint delegates (SOU-229).
pub(crate) struct SettingsListModels {
    window: slint::Weak<MainWindow>,
    dictionary: Rc<VecModel<DictionaryRow>>,
    snippets: Rc<VecModel<SnippetRow>>,
    dictionary_view: Rc<VecModel<DictionaryRow>>,
    snippets_view: Rc<VecModel<SnippetRow>>,
    dictionary_mounted: Cell<(usize, usize)>,
    snippets_mounted: Cell<(usize, usize)>,
}

impl SettingsListModels {
    pub(crate) fn install(window: &MainWindow) -> Rc<Self> {
        let models = Rc::new(Self {
            window: window.as_weak(),
            dictionary: Rc::new(VecModel::default()),
            snippets: Rc::new(VecModel::default()),
            dictionary_view: Rc::new(VecModel::default()),
            snippets_view: Rc::new(VecModel::default()),
            dictionary_mounted: Cell::new((usize::MAX, usize::MAX)),
            snippets_mounted: Cell::new((usize::MAX, usize::MAX)),
        });
        window.set_settings_dictionary_entries(models.dictionary_view.clone().into());
        window.set_settings_snippets(models.snippets_view.clone().into());

        // Slint's expression language can't do the windowing math, so Rust
        // re-windows whenever the Flickable scroll position actually changes
        // (SOU-283: `changed` handlers on the `*-scroll-top` properties in
        // `main_window.slint`, coalesced once per event-loop iteration) -
        // nothing runs while the list sits still or Settings is closed.
        // A data replacement or first mount doesn't fire `changed`, which is
        // why the `populate_*` paths below keep their forced re-window.
        let weak = window.as_weak();
        let models_weak = Rc::downgrade(&models);
        window.on_settings_dictionary_scroll_changed(move |_| {
            if let (Some(window), Some(models)) = (weak.upgrade(), models_weak.upgrade()) {
                models.update_dictionary_window(&window, false);
            }
        });
        let weak = window.as_weak();
        let models_weak = Rc::downgrade(&models);
        window.on_settings_snippets_scroll_changed(move |_| {
            if let (Some(window), Some(models)) = (weak.upgrade(), models_weak.upgrade()) {
                models.update_snippets_window(&window, false);
            }
        });
        models
    }

    pub(crate) fn populate_dictionary(&self, entries: &[DictionaryEntry]) {
        let rows = entries.iter().map(dictionary_row).collect();
        sync_rows_by_id(&self.dictionary, rows, |row| row.id);
        if let Some(window) = self.window.upgrade() {
            self.update_dictionary_window(&window, true);
        }
    }

    pub(crate) fn populate_snippets(&self, entries: &[SnippetEntry], editing_id: Option<i64>) {
        let rows = entries
            .iter()
            .map(|entry| snippet_row(entry, editing_id))
            .collect();
        sync_rows_by_id(&self.snippets, rows, |row| row.id);
        if let Some(window) = self.window.upgrade() {
            self.update_snippets_window(&window, true);
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
            self.update_snippets_window(&window, true);
        }
    }

    /// Recomputes which slice of the full dictionary should be mounted for
    /// the current scroll position and patches the view model, but only when
    /// the slice actually changed (or `force`, after a repopulate).
    fn update_dictionary_window(&self, window: &MainWindow, force: bool) {
        // Flickable's content-y is negative-going-down, same as the
        // transcript's scroll mirror.
        let scroll_top = -window.get_settings_dictionary_scroll_top_px();
        let error_id = window.get_settings_dictionary_delete_error_id();
        let has_error = !window.get_settings_dictionary_delete_error().is_empty();
        let heights: Vec<f32> = self
            .dictionary
            .iter()
            .map(|row| {
                if has_error && row.id == error_id {
                    DICTIONARY_ERROR_ROW_HEIGHT_PX
                } else {
                    DICTIONARY_ROW_HEIGHT_PX
                }
            })
            .collect();
        let offsets = cumulative_offsets(&heights);
        let win = visible_window(
            &offsets,
            scroll_top,
            DICTIONARY_VIEWPORT_HEIGHT_PX,
            DICTIONARY_VIEWPORT_HEIGHT_PX * SETTINGS_LIST_SCROLL_MARGIN_FACTOR,
        );
        if !force && self.dictionary_mounted.get() == (win.start, win.end) {
            return;
        }
        self.dictionary_mounted.set((win.start, win.end));
        let slice: Vec<DictionaryRow> = (win.start..win.end)
            .filter_map(|index| self.dictionary.row_data(index))
            .collect();
        let mounted = slice.len();
        let total = self.dictionary.row_count();
        sync_rows_by_id(&self.dictionary_view, slice, |row| row.id);
        window.set_settings_dictionary_count(total as i32);
        window.set_settings_dictionary_content_height(offsets.last().copied().unwrap_or(0.0));
        window.set_settings_dictionary_spacer_before(win.spacer_before);
        window.set_settings_dictionary_spacer_after(win.spacer_after);
        // SOU-229 AC1/AC2's "delegate counter" evidence: stays small and
        // bounded regardless of how many entries the dictionary has - see
        // `tests::settings_list_windows_stay_bounded_for_0_100_1000_rows`.
        eprintln!("settings dictionary window: {mounted}/{total} rows mounted (SOU-229)");
    }

    /// Snippets counterpart of `update_dictionary_window`.
    fn update_snippets_window(&self, window: &MainWindow, force: bool) {
        let scroll_top = -window.get_settings_snippets_scroll_top_px();
        let error_id = window.get_settings_snippet_delete_error_id();
        let has_error = !window.get_settings_snippet_delete_error().is_empty();
        let heights: Vec<f32> = self
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
            .collect();
        let offsets = cumulative_offsets(&heights);
        let win = visible_window(
            &offsets,
            scroll_top,
            SNIPPETS_VIEWPORT_HEIGHT_PX,
            SNIPPETS_VIEWPORT_HEIGHT_PX * SETTINGS_LIST_SCROLL_MARGIN_FACTOR,
        );
        if !force && self.snippets_mounted.get() == (win.start, win.end) {
            return;
        }
        self.snippets_mounted.set((win.start, win.end));
        let slice: Vec<SnippetRow> = (win.start..win.end)
            .filter_map(|index| self.snippets.row_data(index))
            .collect();
        let mounted = slice.len();
        let total = self.snippets.row_count();
        sync_rows_by_id(&self.snippets_view, slice, |row| row.id);
        window.set_settings_snippets_count(total as i32);
        window.set_settings_snippets_content_height(offsets.last().copied().unwrap_or(0.0));
        window.set_settings_snippets_spacer_before(win.spacer_before);
        window.set_settings_snippets_spacer_after(win.spacer_after);
        eprintln!("settings snippets window: {mounted}/{total} rows mounted (SOU-229)");
    }
}

/// Cumulative y-offsets from per-row heights: `offsets[i]` is the top of row
/// `i`, `offsets[heights.len()]` the total content height - same shape as
/// `transcript::compute_offsets`, which is tied to `TranscriptBlock`.
fn cumulative_offsets(heights: &[f32]) -> Vec<f32> {
    let mut offsets = Vec::with_capacity(heights.len() + 1);
    let mut y = 0.0f32;
    offsets.push(y);
    for height in heights {
        y += height;
        offsets.push(y);
    }
    offsets
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

    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(Default::default()))
        }
    }

    // SOU-283: the Settings lists re-window from the `changed` handlers on
    // their `*-scroll-top` properties, not from a poll timer. Moving the
    // scroll position and letting Slint run its change handlers (what the
    // event loop does once per iteration) must mount a new slice; a
    // repopulate still mounts the top slice on its own.
    #[test]
    fn settings_lists_rewindow_on_scroll_change_without_a_timer() {
        let _ = slint::platform::set_platform(Box::new(TestPlatform));
        let window = MainWindow::new().unwrap();
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
        models.populate_dictionary(&dictionary);
        models.populate_snippets(&snippets, None);

        let first_dictionary_id = || models.dictionary_view.row_data(0).map(|row| row.id);
        let first_snippet_id = || models.snippets_view.row_data(0).map(|row| row.id);
        assert_eq!(first_dictionary_id(), Some(1));
        assert_eq!(first_snippet_id(), Some(1));
        assert_eq!(window.get_settings_dictionary_spacer_before(), 0.0);
        assert_eq!(window.get_settings_snippets_spacer_before(), 0.0);

        // Flickable content-y is negative going down: scroll both lists to
        // the middle of their content.
        window.set_settings_dictionary_scroll_top(-250.0 * DICTIONARY_ROW_HEIGHT_PX);
        window.set_settings_snippets_scroll_top(-250.0 * SNIPPET_ROW_HEIGHT_PX);
        slint::platform::update_timers_and_animations();

        let dictionary_start = first_dictionary_id().expect("dictionary slice mounted");
        let snippets_start = first_snippet_id().expect("snippets slice mounted");
        assert!(
            dictionary_start > 200 && dictionary_start <= 251,
            "dictionary slice should start near row 250, got {dictionary_start}"
        );
        assert!(
            snippets_start > 200 && snippets_start <= 251,
            "snippets slice should start near row 250, got {snippets_start}"
        );
        assert!(window.get_settings_dictionary_spacer_before() > 0.0);
        assert!(window.get_settings_snippets_spacer_before() > 0.0);
        assert!(models.dictionary_view.row_count() < 50);
        assert!(models.snippets_view.row_count() < 50);

        // Back to the top re-windows again.
        window.set_settings_dictionary_scroll_top(0.0);
        window.set_settings_snippets_scroll_top(0.0);
        slint::platform::update_timers_and_animations();
        assert_eq!(first_dictionary_id(), Some(1));
        assert_eq!(first_snippet_id(), Some(1));
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

    // SOU-229 AC1/AC2: with the old ListView-over-the-full-model layout the
    // delegate count was, by construction, exactly the entry count (0/100/
    // 1000 delegates for 0/100/1000 rows - `ListView` is a `ScrollView {
    // @children }`, nothing bounds instantiation). The windowed slice bounds
    // it to viewport + margin regardless of the total.
    #[test]
    fn settings_list_windows_stay_bounded_for_0_100_1000_rows() {
        // Viewport (360px) + a 720px margin on each side = 1800px of rows at
        // 64px each, so about 29 mounted rows, plus the overlap slack
        // `visible_window` allows on both edges.
        let max_mounted = (((1.0 + 2.0 * SETTINGS_LIST_SCROLL_MARGIN_FACTOR)
            * DICTIONARY_VIEWPORT_HEIGHT_PX)
            / DICTIONARY_ROW_HEIGHT_PX)
            .ceil() as usize
            + 2;
        for total in [0usize, 100, 1000] {
            let heights = vec![DICTIONARY_ROW_HEIGHT_PX; total];
            let offsets = cumulative_offsets(&heights);
            let content_height = offsets.last().copied().unwrap_or(0.0);
            // Probe the top, the middle, and the bottom of the scroll range.
            for scroll_top in [0.0, content_height / 2.0, content_height] {
                let win = visible_window(
                    &offsets,
                    scroll_top,
                    DICTIONARY_VIEWPORT_HEIGHT_PX,
                    DICTIONARY_VIEWPORT_HEIGHT_PX * SETTINGS_LIST_SCROLL_MARGIN_FACTOR,
                );
                let mounted = win.end - win.start;
                assert!(
                    mounted <= max_mounted.min(total.max(1)),
                    "{mounted} rows mounted for {total} entries at scroll {scroll_top}"
                );
                // The spacers stand in for everything not mounted, so the
                // scrollbar still reflects the full list.
                let mounted_height: f32 = heights[win.start..win.end].iter().sum();
                let reconstructed = win.spacer_before + mounted_height + win.spacer_after;
                assert!(
                    (reconstructed - content_height).abs() < 0.5,
                    "spacers + slice should reconstruct the full {content_height}px, got {reconstructed}"
                );
            }
        }

        // A short list still mounts everything - windowing must not clip a
        // list that fits on screen.
        let heights = vec![DICTIONARY_ROW_HEIGHT_PX; 5];
        let offsets = cumulative_offsets(&heights);
        let win = visible_window(
            &offsets,
            0.0,
            DICTIONARY_VIEWPORT_HEIGHT_PX,
            DICTIONARY_VIEWPORT_HEIGHT_PX * SETTINGS_LIST_SCROLL_MARGIN_FACTOR,
        );
        assert_eq!((win.start, win.end), (0, 5));
        assert_eq!(win.spacer_before, 0.0);
        assert_eq!(win.spacer_after, 0.0);
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
