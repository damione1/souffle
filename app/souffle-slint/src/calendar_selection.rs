//! The rules of the calendar choices, without any I/O: which calendars a stored
//! selection covers, how a checkbox changes it, and how the rows are projected.
use std::rc::Rc;

use slint::{Model, VecModel};
use souffle_lib::calendar::CalendarInfo;

use crate::{CalendarRow, MainWindow};

/// Why a checkbox gesture does not change the stored selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    UnknownCalendar,
    LastCalendar,
    AlreadyApplied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SelectionChange {
    Write(Vec<String>),
    Unchanged(Refusal),
}

/// The calendars a stored selection really covers. An empty list means all of
/// them; an identifier that is no longer in the catalogue covers nothing.
fn effective_selection<'a>(stored: &[String], catalogue: &'a [CalendarInfo]) -> Vec<&'a str> {
    catalogue
        .iter()
        .map(|calendar| calendar.id.as_str())
        .filter(|id| stored.is_empty() || stored.iter().any(|stored| stored == id))
        .collect()
}

/// Sets one calendar to the state the user asked for, by identity. The result
/// is the new stored list: empty when every calendar of the catalogue ends up
/// selected, otherwise the retained identifiers in their stored order followed
/// by the new one. Identifiers of calendars that are currently absent stay
/// stored, so a calendar that comes back keeps its choice.
pub(crate) fn change_selection(
    stored: &[String],
    catalogue: &[CalendarInfo],
    id: &str,
    selected: bool,
) -> SelectionChange {
    if !catalogue.iter().any(|calendar| calendar.id == id) {
        return SelectionChange::Unchanged(Refusal::UnknownCalendar);
    }
    let effective = effective_selection(stored, catalogue);
    let is_selected = effective.contains(&id);
    if is_selected == selected {
        return SelectionChange::Unchanged(Refusal::AlreadyApplied);
    }
    let next: Vec<&str> = if selected {
        catalogue
            .iter()
            .map(|calendar| calendar.id.as_str())
            .filter(|candidate| *candidate == id || effective.contains(candidate))
            .collect()
    } else {
        effective
            .iter()
            .copied()
            .filter(|candidate| *candidate != id)
            .collect()
    };
    if next.is_empty() {
        return SelectionChange::Unchanged(Refusal::LastCalendar);
    }
    if next.len() == catalogue.len() {
        return SelectionChange::Write(Vec::new());
    }
    let is_stale = |id: &String| !catalogue.iter().any(|calendar| &calendar.id == id);
    let mut written: Vec<String> = stored
        .iter()
        .filter(|id| is_stale(id) || next.contains(&id.as_str()))
        .cloned()
        .collect();
    for id in next {
        if !written.iter().any(|existing| existing == id) {
            written.push(id.to_string());
        }
    }
    SelectionChange::Write(written)
}

/// Rows in the order they are shown: by account then title, and by identifier
/// for two calendars with the same name.
pub(crate) fn rows(calendars: &[CalendarInfo], stored: &[String]) -> Vec<CalendarRow> {
    let mut sorted: Vec<&CalendarInfo> = calendars.iter().collect();
    sorted.sort_by(|a, b| {
        a.source_title
            .as_deref()
            .unwrap_or("")
            .cmp(b.source_title.as_deref().unwrap_or(""))
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut last_source: Option<&str> = None;
    sorted
        .into_iter()
        .map(|calendar| {
            let source = calendar.source_title.as_deref().unwrap_or("");
            let show_header = last_source != Some(source);
            last_source = Some(source);
            CalendarRow {
                id: calendar.id.as_str().into(),
                title: calendar.title.as_str().into(),
                source_title: source.into(),
                selected: stored.is_empty() || stored.iter().any(|id| id == &calendar.id),
                show_header,
            }
        })
        .collect()
}

/// Updates the displayed rows in place when the same calendars are shown in the
/// same order, so that delegates (and a focused checkbox) survive a refresh.
pub(crate) fn publish_rows(window: &MainWindow, rows: Vec<CalendarRow>) {
    let current = window.get_settings_calendars();
    let Some(model) = current.as_any().downcast_ref::<VecModel<CalendarRow>>() else {
        window.set_settings_calendars(Rc::new(VecModel::from(rows)).into());
        return;
    };
    let same_calendars = model.row_count() == rows.len()
        && rows.iter().enumerate().all(|(index, row)| {
            model
                .row_data(index)
                .is_some_and(|shown| shown.id == row.id)
        });
    if !same_calendars {
        model.set_vec(rows);
        return;
    }
    for (index, row) in rows.into_iter().enumerate() {
        if model.row_data(index).as_ref() != Some(&row) {
            model.set_row_data(index, row);
        }
    }
}
