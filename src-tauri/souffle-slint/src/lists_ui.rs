//! Dictionary + Snippets sections (SOU-188 milestone 6). Both are plain CRUD
//! lists backed by dedicated sync commands (`add_dictionary_entry`,
//! `add_snippet`, ...), not a full-`AppSettings` round trip - no bounds or
//! state machine to consume here, just row formatting.

use crate::{DictionaryRow, MainWindow, SnippetRow};
use souffle_lib::db::snippets::SnippetEntry;
use souffle_lib::filter::DictionaryEntry;

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
