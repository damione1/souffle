//! Port of features/timeline/controller.svelte.ts's pure functions
//! (toTimelineItems, groupByDay, day label formatting). Everything else in
//! that controller (search debounce, FTS result matching) is not wired yet -
//! see the "not wired yet" comments in main.rs.

use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use souffle_lib::calendar::CalendarEvent;
use souffle_lib::db::dictation::DictationEntry;
use souffle_lib::transcript::MeetingListItem;

use crate::{
    HistoryEntry, TimelineDay, TimelineDayGroup, TimelineFilter, TimelineKind, UpcomingEvent,
    UpcomingPhase,
};

pub fn format_duration(seconds: f64) -> String {
    let total = seconds.round().max(0.0) as i64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn time_label(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%H:%M").to_string()
}

fn day_key(at: DateTime<Utc>) -> NaiveDate {
    at.with_timezone(&Local).date_naive()
}

/// Header of a day group relative to `today`. The wording lives in
/// timeline_section.slint behind @tr(), so Rust only ships which kind of
/// day it is plus the date parts (SOU-246).
fn day_group(day: NaiveDate, today: NaiveDate, entries: Vec<HistoryEntry>) -> TimelineDayGroup {
    let kind = if day == today {
        TimelineDay::Today
    } else if Some(day) == today.pred_opt() {
        TimelineDay::Yesterday
    } else {
        TimelineDay::Earlier
    };
    TimelineDayGroup {
        day: kind,
        weekday: day.weekday().num_days_from_monday() as i32,
        day_of_month: day.day() as i32,
        month: day.month0() as i32,
        entries: std::rc::Rc::new(slint::VecModel::from(entries)).into(),
    }
}

/// One merged, sorted (newest first) entry per dictation/meeting - mirrors
/// `toTimelineItems`. Returns entries paired with their sort/group key so
/// the caller doesn't have to re-parse the timestamp.
fn merge_entries(
    dictations: &[DictationEntry],
    meetings: &[MeetingListItem],
) -> Vec<(DateTime<Utc>, HistoryEntry)> {
    let mut entries: Vec<(DateTime<Utc>, HistoryEntry)> =
        Vec::with_capacity(dictations.len() + meetings.len());

    for entry in dictations {
        let Ok(at) = DateTime::parse_from_rfc3339(&entry.timestamp) else {
            continue;
        };
        let at = at.with_timezone(&Utc);
        entries.push((
            at,
            HistoryEntry {
                kind: TimelineKind::Dictation,
                id: entry.id.clone().into(),
                title: entry.text.clone().into(),
                time_label: time_label(at).into(),
                duration_label: "".into(),
                has_summary: false,
                summary_is_stale: false,
            },
        ));
    }

    for meeting in meetings {
        entries.push((
            meeting.started_at,
            HistoryEntry {
                kind: TimelineKind::Meeting,
                id: meeting.id.clone().into(),
                title: meeting.title.clone().into(),
                time_label: time_label(meeting.started_at).into(),
                duration_label: format_duration(meeting.duration_seconds).into(),
                has_summary: meeting.has_summary,
                summary_is_stale: meeting.summary_is_stale,
            },
        ));
    }

    entries.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    entries
}

/// Kind filter ("all" | "meeting" | "dictation") + substring title search -
/// mirrors `currentFilteredItems` minus the FTS branch (search_text is not
/// wired here yet; substring match on title is the same fallback the
/// Svelte controller uses when there's no FTS hit).
pub fn build_groups(
    dictations: &[DictationEntry],
    meetings: &[MeetingListItem],
    kind_filter: TimelineFilter,
    search_query: &str,
) -> Vec<TimelineDayGroup> {
    let query = search_query.trim().to_lowercase();
    let entries = merge_entries(dictations, meetings);

    let matches_filter = |kind: TimelineKind| match kind_filter {
        TimelineFilter::All => true,
        TimelineFilter::Meeting => kind == TimelineKind::Meeting,
        TimelineFilter::Dictation => kind == TimelineKind::Dictation,
    };

    let mut groups: Vec<(NaiveDate, Vec<HistoryEntry>)> = Vec::new();
    for (at, entry) in entries {
        if !matches_filter(entry.kind) {
            continue;
        }
        if !query.is_empty() && !entry.title.to_lowercase().contains(&query) {
            continue;
        }
        let day = day_key(at);
        match groups.last_mut() {
            Some((last_day, items)) if *last_day == day => items.push(entry),
            _ => groups.push((day, vec![entry])),
        }
    }

    let today = Local::now().date_naive();
    groups
        .into_iter()
        .map(|(day, entries)| day_group(day, today, entries))
        .collect()
}

/// Entries on one page of the history. The list is not virtualized, so this
/// bounds what a mount, a scroll or a page change costs whatever the size of
/// the history: 50 rows mount in about 25 ms in a debug build.
pub const HISTORY_PAGE_SIZE: usize = 50;

/// One page of a history and where it sits in the whole of it.
pub struct HistoryPage {
    pub groups: Vec<TimelineDayGroup>,
    pub page: usize,
    pub page_count: usize,
    /// Entries on every page.
    pub total: usize,
}

/// Cuts `groups` (the whole filtered history, newest first) into pages of
/// `size` entries and returns the one at `requested`, or the last one when
/// the history no longer reaches that far. A day cut by a page boundary
/// keeps its heading on both pages.
pub fn page_of(groups: Vec<TimelineDayGroup>, requested: usize, size: usize) -> HistoryPage {
    use slint::Model;
    let size = size.max(1);
    let total: usize = groups.iter().map(|group| group.entries.row_count()).sum();
    let page_count = total.div_ceil(size);
    let page = requested.min(page_count.saturating_sub(1));
    let (first, end) = (page * size, (page + 1) * size);
    let mut seen = 0;
    let mut shown = Vec::new();
    for group in groups {
        let count = group.entries.row_count();
        let (from, to) = (first.max(seen), end.min(seen + count));
        if from < to {
            let entries: Vec<HistoryEntry> = group
                .entries
                .iter()
                .skip(from - seen)
                .take(to - from)
                .collect();
            shown.push(TimelineDayGroup {
                entries: std::rc::Rc::new(slint::VecModel::from(entries)).into(),
                ..group
            });
        }
        seen += count;
    }
    HistoryPage {
        groups: shown,
        page,
        page_count,
        total,
    }
}

/// Shows the page of `groups` the window is on. `groups` is the whole
/// filtered history: the page stays where it is across a refresh, and moves
/// back only when the history got too short for it.
pub fn show_history(window: &crate::MainWindow, groups: Vec<TimelineDayGroup>) {
    show_history_paged(window, groups, HISTORY_PAGE_SIZE);
}

pub fn show_history_paged(window: &crate::MainWindow, groups: Vec<TimelineDayGroup>, size: usize) {
    let requested = window.get_timeline_page().max(0) as usize;
    let page = page_of(groups, requested, size);
    window.set_timeline_page(page.page as i32);
    window.set_timeline_page_count(page.page_count as i32);
    window.set_timeline_total(page.total as i32);
    show_groups(window, page.groups);
}

/// Shows `groups` in the history, changing only what differs from what is
/// already there. A search key, a filter or a finished recording refreshes
/// the whole history: installing a new model each time would rebuild every
/// row, where most of them are the same entries as before.
pub fn show_groups(window: &crate::MainWindow, groups: Vec<TimelineDayGroup>) {
    use slint::Model;
    let current = window.get_timeline_groups();
    match current
        .as_any()
        .downcast_ref::<slint::VecModel<TimelineDayGroup>>()
    {
        Some(model) => sync_groups(model, groups),
        None => window.set_timeline_groups(std::rc::Rc::new(slint::VecModel::from(groups)).into()),
    }
}

/// Brings `model` to `target` with row-level changes: rows whose `key` is
/// already there stay (and are passed to `update`), the others are inserted
/// or removed. Both lists are in the same order, newest first.
fn sync_rows<T: Clone + 'static, K: PartialEq>(
    model: &slint::VecModel<T>,
    target: Vec<T>,
    key: impl Fn(&T) -> K,
    update: impl Fn(usize, &T, T),
) {
    use slint::Model;
    let count = target.len();
    for (index, row) in target.into_iter().enumerate() {
        let wanted = key(&row);
        let found = (index..model.row_count()).find(|at| {
            model
                .row_data(*at)
                .is_some_and(|current| key(&current) == wanted)
        });
        match found {
            Some(at) => {
                // The rows before it are gone from the target.
                for _ in index..at {
                    model.remove(index);
                }
                let current = model.row_data(index).expect("the row just found");
                update(index, &current, row);
            }
            None => model.insert(index, row),
        }
    }
    while model.row_count() > count {
        model.remove(model.row_count() - 1);
    }
}

fn sync_groups(model: &slint::VecModel<TimelineDayGroup>, groups: Vec<TimelineDayGroup>) {
    use slint::Model;
    sync_rows(
        model,
        groups,
        |group| (group.day, group.weekday, group.day_of_month, group.month),
        |index, current, group| {
            // The day keeps its entry model, so its rows are not rebuilt.
            match current
                .entries
                .as_any()
                .downcast_ref::<slint::VecModel<HistoryEntry>>()
            {
                Some(entries) => sync_rows(
                    entries,
                    group.entries.iter().collect(),
                    |entry| (entry.kind, entry.id.clone()),
                    |at, shown, entry| {
                        if *shown != entry {
                            entries.set_row_data(at, entry);
                        }
                    },
                ),
                None => model.set_row_data(index, group),
            }
        },
    );
}

pub fn occurrence_key(event: &CalendarEvent) -> String {
    format!("{}-{}", event.id, event.start.to_rfc3339())
}

/// Port of TimelineSection.svelte's `eventPhase` / `timeLabel`. `now` is
/// injected so a 30s poll can refresh "en cours" / "suivant" without the
/// UI parsing timestamps.
pub fn upcoming_rows(events: &[CalendarEvent], now: DateTime<Utc>) -> Vec<UpcomingEvent> {
    let first_upcoming = events
        .iter()
        .find(|event| event.start > now)
        .map(occurrence_key);

    events
        .iter()
        .map(|event| {
            let phase = if now >= event.end {
                UpcomingPhase::Past
            } else if now >= event.start {
                UpcomingPhase::Now
            } else if first_upcoming.as_deref() == Some(occurrence_key(event).as_str()) {
                UpcomingPhase::Next
            } else {
                UpcomingPhase::Later
            };
            UpcomingEvent {
                id: occurrence_key(event).into(),
                title: event.title.as_str().into(),
                time_range: format!("{}–{}", time_label(event.start), time_label(event.end)).into(),
                participant_count: i32::try_from(event.participants.len()).unwrap_or(i32::MAX),
                phase,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccessState, MainWindow};
    use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementRoot};
    use slint::ComponentHandle;
    use slint::platform::{Platform, WindowAdapter, software_renderer::MinimalSoftwareWindow};
    use std::rc::Rc;

    const WIDTH: u32 = 1040;
    const HEIGHT: u32 = 1280;

    thread_local! {
        static TEST_WINDOW: std::cell::RefCell<Option<Rc<MinimalSoftwareWindow>>> =
            const { std::cell::RefCell::new(None) };
    }

    struct TestPlatform;

    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            let window = MinimalSoftwareWindow::new(Default::default());
            TEST_WINDOW.with(|slot| *slot.borrow_mut() = Some(window.clone()));
            Ok(window)
        }
    }

    fn settle() {
        let window = TEST_WINDOW
            .with(|slot| slot.borrow().clone())
            .expect("no test window");
        let size = window.size();
        let mut frame = vec![
            slint::platform::software_renderer::Rgb565Pixel::default();
            (size.width * size.height) as usize
        ];
        for _ in 0..3 {
            slint::platform::update_timers_and_animations();
            window.request_redraw();
            window.draw_if_needed(|renderer| {
                renderer.render(&mut frame, size.width as usize);
            });
        }
    }

    fn timeline_window() -> MainWindow {
        let _ = slint::platform::set_platform(Box::new(TestPlatform));
        let window = crate::settings_kit::new_window().unwrap();
        window
            .window()
            .set_size(slint::PhysicalSize::new(WIDTH, HEIGHT));
        window.set_settings_calendar_enabled(true);
        window.set_settings_calendar_permission(AccessState::Granted);
        window.set_timeline_is_empty(false);
        window.set_timeline_has_matches(true);

        let entries: Vec<_> = (0..30)
            .map(|index| HistoryEntry {
                kind: TimelineKind::Meeting,
                id: format!("meeting-{index}").into(),
                title: format!("Réunion {index}").into(),
                time_label: "10:00".into(),
                duration_label: "32:10".into(),
                has_summary: false,
                summary_is_stale: false,
            })
            .collect();
        let group = TimelineDayGroup {
            day: TimelineDay::Today,
            weekday: 0,
            day_of_month: 25,
            month: 8,
            entries: slint::ModelRc::new(slint::VecModel::from(entries)),
        };
        window.set_timeline_groups(slint::ModelRc::new(slint::VecModel::from(vec![group])));
        settle();
        window
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
    }

    fn click(window: &MainWindow, element: &ElementHandle) {
        let position = slint::LogicalPosition::new(
            element.absolute_position().x + element.size().width / 2.0,
            element.absolute_position().y + element.size().height / 2.0,
        );
        let button = slint::platform::PointerEventButton::Left;
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerPressed { position, button });
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button });
    }

    #[test]
    fn owned_timeline_action_exposes_reason_and_rejects_ax_and_pointer() {
        use crate::ActionGuards;
        use souffle_lib::meeting_access::MeetingDeleteBlockReason;
        let window = timeline_window();
        window.global::<ActionGuards>().on_meeting_delete(|id, _| {
            crate::action_guards::permission(
                (id == "meeting-0").then_some(MeetingDeleteBlockReason::Finalizing),
            )
        });
        window.global::<ActionGuards>().set_revision(1);
        let removed = Rc::new(std::cell::Cell::new(0));
        let seen = Rc::clone(&removed);
        window.on_timeline_item_removed(move |_, _| seen.set(seen.get() + 1));
        settle();
        let action = window
            .root_element()
            .query_descendants()
            .match_predicate(|el| {
                el.accessible_role() == Some(AccessibleRole::Button)
                    && el.accessible_label().is_some_and(|label| {
                        label.contains("finalisation") || label.contains("finalizing")
                    })
            })
            .find_first()
            .expect("disabled delete action with localized reason");
        assert_eq!(action.accessible_enabled(), Some(false));
        action.invoke_accessible_default_action();
        click(&window, &action);
        settle();
        assert_eq!(removed.get(), 0);
        let old_action = window
            .root_element()
            .query_descendants()
            .match_predicate(|el| {
                el.accessible_role() == Some(AccessibleRole::Button)
                    && el
                        .accessible_label()
                        .is_some_and(|label| label == "Supprimer" || label == "Delete")
            })
            .find_first()
            .unwrap();
        old_action.invoke_accessible_default_action();
        settle();
        let confirm = window
            .root_element()
            .query_descendants()
            .match_predicate(|el| {
                el.accessible_label().is_some_and(|label| {
                    label == "Confirmer la suppression" || label == "Confirm deletion"
                })
            })
            .find_first()
            .unwrap();
        confirm.invoke_accessible_default_action();
        assert_eq!(
            removed.get(),
            1,
            "a distinct archived meeting stays deletable"
        );
    }

    #[test]
    fn owned_detail_action_rejects_ax_pointer_and_keyboard() {
        use crate::ActionGuards;
        use souffle_lib::meeting_access::MeetingDeleteBlockReason;
        let window = timeline_window();
        window.global::<ActionGuards>().on_meeting_delete(|_, _| {
            crate::action_guards::permission(Some(MeetingDeleteBlockReason::Recording))
        });
        window.set_active_meeting_id("meeting-0".into());
        window.set_meeting_detail_title("Protected meeting".into());
        let removed = Rc::new(std::cell::Cell::new(0));
        let seen = Rc::clone(&removed);
        window.on_meeting_detail_delete(move || seen.set(seen.get() + 1));
        settle();
        let action = window
            .root_element()
            .query_descendants()
            .match_predicate(|el| {
                el.accessible_role() == Some(AccessibleRole::Button)
                    && el.accessible_label().is_some_and(|label| {
                        label == "Supprimer la réunion" || label == "Delete meeting"
                    })
            })
            .find_first()
            .expect("detail delete control");
        assert_eq!(action.accessible_enabled(), Some(false));
        action.invoke_accessible_default_action();
        click(&window, &action);
        for key in [slint::platform::Key::Return, slint::platform::Key::Space] {
            window
                .window()
                .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: key.into() });
        }
        settle();
        assert_eq!(removed.get(), 0);
        assert_eq!(window.get_active_meeting_id(), "meeting-0");
    }

    fn history_entry(id: &str, title: &str) -> HistoryEntry {
        HistoryEntry {
            kind: TimelineKind::Meeting,
            id: id.into(),
            title: title.into(),
            ..Default::default()
        }
    }

    fn history_day(day_of_month: i32, entries: &[(&str, &str)]) -> TimelineDayGroup {
        TimelineDayGroup {
            day: TimelineDay::Earlier,
            day_of_month,
            entries: slint::ModelRc::new(slint::VecModel::from(
                entries
                    .iter()
                    .map(|(id, title)| history_entry(id, title))
                    .collect::<Vec<_>>(),
            )),
            ..Default::default()
        }
    }

    fn shown(model: &slint::VecModel<TimelineDayGroup>) -> Vec<(i32, Vec<String>)> {
        use slint::Model;
        model
            .iter()
            .map(|group| {
                (
                    group.day_of_month,
                    group
                        .entries
                        .iter()
                        .map(|entry| format!("{}:{}", entry.id, entry.title))
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn a_refresh_changes_the_history_in_place_and_keeps_the_days_that_remain() {
        use slint::Model;
        let full = || {
            vec![
                history_day(9, &[("a", "Alpha"), ("b", "Beta"), ("c", "Gamma")]),
                history_day(8, &[("d", "Delta")]),
                history_day(7, &[("e", "Epsilon"), ("f", "Zeta")]),
            ]
        };
        let model = slint::VecModel::from(full());
        let day_nine = model.row_data(0).unwrap().entries;
        let day_seven = model.row_data(2).unwrap().entries;

        // The same entries: nothing is replaced.
        sync_groups(&model, full());
        assert!(model.row_data(0).unwrap().entries == day_nine);

        // A search narrows two days and drops one.
        let narrowed = vec![
            history_day(9, &[("b", "Beta")]),
            history_day(7, &[("f", "Zeta")]),
        ];
        sync_groups(&model, narrowed);
        assert_eq!(
            shown(&model),
            [
                (9, vec!["b:Beta".to_string()]),
                (7, vec!["f:Zeta".to_string()])
            ]
        );
        assert!(model.row_data(0).unwrap().entries == day_nine);
        assert!(model.row_data(1).unwrap().entries == day_seven);

        // Clearing it restores everything in order, a renamed entry included.
        let mut restored = full();
        restored[0] = history_day(9, &[("a", "Alpha"), ("b", "Beta renamed"), ("c", "Gamma")]);
        restored.insert(0, history_day(10, &[("g", "Eta")]));
        sync_groups(&model, restored);
        assert_eq!(
            shown(&model),
            [
                (10, vec!["g:Eta".to_string()]),
                (
                    9,
                    vec![
                        "a:Alpha".to_string(),
                        "b:Beta renamed".to_string(),
                        "c:Gamma".to_string()
                    ]
                ),
                (8, vec!["d:Delta".to_string()]),
                (7, vec!["e:Epsilon".to_string(), "f:Zeta".to_string()]),
            ]
        );
        assert!(model.row_data(1).unwrap().entries == day_nine);

        sync_groups(&model, Vec::new());
        assert_eq!(model.row_count(), 0);
    }

    fn numbered_day(day_of_month: i32, entries: std::ops::Range<usize>) -> TimelineDayGroup {
        let entries: Vec<(String, String)> = entries
            .map(|index| (format!("m{index}"), format!("Réunion {index}")))
            .collect();
        history_day(
            day_of_month,
            &entries
                .iter()
                .map(|(id, title)| (id.as_str(), title.as_str()))
                .collect::<Vec<_>>(),
        )
    }

    fn ids(groups: &[TimelineDayGroup]) -> Vec<(i32, Vec<String>)> {
        use slint::Model;
        groups
            .iter()
            .map(|group| {
                (
                    group.day_of_month,
                    group
                        .entries
                        .iter()
                        .map(|entry| entry.id.to_string())
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn a_day_cut_by_a_page_keeps_its_heading_on_both_pages() {
        let history = || {
            vec![
                numbered_day(9, 0..4),
                numbered_day(8, 4..7),
                numbered_day(7, 7..12),
            ]
        };
        let label = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();

        let first = page_of(history(), 0, 5);
        assert_eq!((first.page, first.page_count, first.total), (0, 3, 12));
        assert_eq!(
            ids(&first.groups),
            [(9, label(&["m0", "m1", "m2", "m3"])), (8, label(&["m4"]))]
        );
        let second = page_of(history(), 1, 5);
        assert_eq!(
            ids(&second.groups),
            [(8, label(&["m5", "m6"])), (7, label(&["m7", "m8", "m9"]))]
        );
        let last = page_of(history(), 2, 5);
        assert_eq!(ids(&last.groups), [(7, label(&["m10", "m11"]))]);
    }

    #[test]
    fn a_page_past_the_end_falls_back_to_the_last_one_and_never_shows_nothing() {
        // The only entry of page 3 was deleted: ten entries are left.
        let after_delete = page_of(vec![numbered_day(9, 0..10)], 2, 5);
        assert_eq!((after_delete.page, after_delete.page_count), (1, 2));
        assert_eq!(ids(&after_delete.groups)[0].1.len(), 5);

        let one_page = page_of(vec![numbered_day(9, 0..5)], 4, 5);
        assert_eq!(
            (one_page.page, one_page.page_count, one_page.total),
            (0, 1, 5)
        );

        let empty = page_of(Vec::new(), 3, 5);
        assert_eq!((empty.page, empty.page_count, empty.total), (0, 0, 0));
        assert!(empty.groups.is_empty());
    }

    fn rows(window: &MainWindow) -> usize {
        ElementHandle::find_by_element_type_name(window, "TimelineItemRow").count()
    }

    fn pager(window: &MainWindow) -> Option<ElementHandle> {
        ElementHandle::find_by_element_id(window, "IdleView::history-pages").next()
    }

    /// Previous and Next, in that order.
    fn pager_buttons(window: &MainWindow) -> Vec<ElementHandle> {
        pager(window)
            .expect("the pager is shown")
            .query_descendants()
            .match_inherits("KitButton")
            .find_all()
    }

    #[test]
    fn a_refresh_stays_on_its_page_and_a_shorter_history_moves_it_back() {
        let window = timeline_window();
        window
            .window()
            .set_size(slint::PhysicalSize::new(WIDTH, 4000));
        show_history_paged(&window, vec![numbered_day(9, 0..23)], 10);
        settle();
        assert_eq!(
            (
                window.get_timeline_page(),
                window.get_timeline_page_count(),
                window.get_timeline_total()
            ),
            (0, 3, 23)
        );
        assert_eq!(rows(&window), 10, "only the page is instantiated");

        window.set_timeline_page(2);
        show_history_paged(&window, vec![numbered_day(9, 0..23)], 10);
        settle();
        assert_eq!(rows(&window), 3);

        // The same history again, as after a finished recording.
        show_history_paged(&window, vec![numbered_day(9, 0..23)], 10);
        assert_eq!(window.get_timeline_page(), 2);

        // The three entries of the last page are deleted one after the other.
        show_history_paged(&window, vec![numbered_day(9, 0..21)], 10);
        assert_eq!(window.get_timeline_page(), 2);
        show_history_paged(&window, vec![numbered_day(9, 0..20)], 10);
        settle();
        assert_eq!(
            (window.get_timeline_page(), window.get_timeline_page_count()),
            (1, 2)
        );
        assert_eq!(rows(&window), 10, "the page before it, not an empty one");
    }

    #[test]
    fn the_pager_shows_only_for_several_pages_and_asks_for_the_next_one() {
        let window = timeline_window();
        window
            .window()
            .set_size(slint::PhysicalSize::new(WIDTH, 4000));
        let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = asked.clone();
        let weak = window.as_weak();
        window.on_timeline_page_changed(move |page| {
            sink.borrow_mut().push(page);
            // The window already moved: the app only loads that page.
            let window = weak.unwrap();
            assert_eq!(window.get_timeline_page(), page);
            show_history_paged(&window, vec![numbered_day(9, 0..23)], 10);
        });

        show_history_paged(&window, vec![numbered_day(9, 0..10)], 10);
        settle();
        assert!(pager(&window).is_none(), "one page needs no pager");

        show_history_paged(&window, vec![numbered_day(9, 0..23)], 10);
        settle();
        let buttons = pager_buttons(&window);
        assert_eq!(buttons.len(), 2);
        assert_eq!(buttons[0].accessible_enabled(), Some(false));
        buttons[1].invoke_accessible_default_action();
        settle();
        assert_eq!(*asked.borrow(), [1]);
        assert_eq!(window.get_timeline_page(), 1);
        assert_eq!(rows(&window), 10);

        let buttons = pager_buttons(&window);
        buttons[1].invoke_accessible_default_action();
        settle();
        assert_eq!(*asked.borrow(), [1, 2]);
        assert_eq!(rows(&window), 3);
        assert_eq!(
            pager_buttons(&window)[1].accessible_enabled(),
            Some(false),
            "no page after the last"
        );
        pager_buttons(&window)[0].invoke_accessible_default_action();
        settle();
        assert_eq!(window.get_timeline_page(), 1);
    }

    #[test]
    fn a_filter_or_a_search_starts_again_on_the_first_page() {
        let window = timeline_window();
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = seen.clone();
        let weak = window.as_weak();
        window
            .on_filter_changed(move |_| sink.borrow_mut().push(weak.unwrap().get_timeline_page()));
        let sink = seen.clone();
        let weak = window.as_weak();
        window
            .on_search_changed(move |_| sink.borrow_mut().push(weak.unwrap().get_timeline_page()));

        window.set_timeline_page(2);
        show_history_paged(&window, vec![numbered_day(9, 0..23)], 10);
        settle();
        let chips: Vec<_> =
            ElementHandle::find_by_element_type_name(&window, "KitChipButton").collect();
        assert_eq!(chips.len(), 3);
        chips[1].invoke_accessible_default_action();
        assert_eq!(
            *seen.borrow(),
            [0],
            "the handler already sees the first page"
        );

        window.set_timeline_page(2);
        window.invoke_search_changed("budget".into());
        assert_eq!(
            *seen.borrow(),
            [0, 2],
            "a search typed in the field goes through the view"
        );
    }

    #[test]
    fn a_page_change_returns_to_the_top_of_the_history() {
        let window = timeline_window();
        show_history_paged(&window, vec![numbered_day(9, 0..60)], 20);
        settle();
        let top = || {
            ElementHandle::find_by_element_id(&window, "IdleView::history-content")
                .next()
                .unwrap()
                .absolute_position()
                .y
        };
        let rest = top();
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(520.0, 600.0),
                delta_x: 0.0,
                delta_y: -400.0,
            });
        settle();
        assert!(top() < rest - 300.0, "the history scrolled");
        window.set_timeline_page(1);
        show_history_paged(&window, vec![numbered_day(9, 0..60)], 20);
        settle();
        assert_eq!(top(), rest);
    }

    #[test]
    fn day_group_marks_today_and_yesterday_relative_to_today() {
        let today = date(2026, 9, 23);
        assert_eq!(day_group(today, today, vec![]).day, TimelineDay::Today);
        assert_eq!(
            day_group(date(2026, 9, 22), today, vec![]).day,
            TimelineDay::Yesterday
        );
        // Across a month boundary too.
        assert_eq!(
            day_group(date(2026, 9, 30), date(2026, 10, 1), vec![]).day,
            TimelineDay::Yesterday
        );
    }

    #[test]
    fn earlier_day_ships_date_parts_for_the_slint_label() {
        // Monday 21 September 2026: Slint indexes weekday 0 = Monday and
        // month 0 = January into its @tr() name lists.
        let group = day_group(date(2026, 9, 21), date(2026, 9, 23), vec![]);
        assert_eq!(group.day, TimelineDay::Earlier);
        assert_eq!(group.weekday, 0);
        assert_eq!(group.day_of_month, 21);
        assert_eq!(group.month, 8);
    }

    #[test]
    fn clicking_the_empty_timeline_gutter_cancels_delete_confirmation() {
        let window = timeline_window();
        let removed: Rc<std::cell::RefCell<Vec<String>>> = Rc::default();
        let removed_sink = removed.clone();
        window.on_timeline_item_removed(move |_kind, id| {
            removed_sink.borrow_mut().push(id.into());
        });
        let row = ElementHandle::find_by_accessible_label(&window, "Réunion 0")
            .next()
            .expect("no timeline row");
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(
                    row.absolute_position().x + row.size().width / 2.0,
                    row.absolute_position().y + row.size().height / 2.0,
                ),
            });
        settle();
        let delete = window
            .root_element()
            .query_descendants()
            .match_predicate(|element| {
                element.accessible_role() == Some(AccessibleRole::Button)
                    && element
                        .accessible_label()
                        .is_some_and(|label| label == "Supprimer" || label == "Delete")
            })
            .find_first()
            .expect("no timeline delete button");
        delete.invoke_accessible_default_action();
        settle();
        assert!(
            window
                .root_element()
                .query_descendants()
                .match_predicate(|element| {
                    element.accessible_label().is_some_and(|label| {
                        label == "Confirmer la suppression" || label == "Confirm deletion"
                    })
                })
                .find_first()
                .is_some(),
            "delete confirmation did not open"
        );

        // The six-pixel gap above the first row is inside the centered,
        // scrollable history viewport but outside every row TouchArea.
        let position = slint::LogicalPosition::new(
            row.absolute_position().x + row.size().width / 2.0,
            row.absolute_position().y - 3.0,
        );
        let button = slint::platform::PointerEventButton::Left;
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerPressed { position, button });
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button });
        settle();

        assert!(
            window
                .root_element()
                .query_descendants()
                .match_predicate(|element| {
                    element.accessible_label().is_some_and(|label| {
                        label == "Confirmer la suppression" || label == "Confirm deletion"
                    })
                })
                .find_first()
                .is_none(),
            "empty-gutter click left delete confirmation open"
        );
        assert!(
            window
                .root_element()
                .query_descendants()
                .match_predicate(|element| {
                    element
                        .accessible_label()
                        .is_some_and(|label| label == "Supprimer" || label == "Delete")
                })
                .find_first()
                .is_some(),
            "delete action did not return after cancellation"
        );

        // Re-open confirmation, then use the actual pointer path on the
        // button. The page underlay must not clear the state before the
        // button's removal callback runs.
        delete.invoke_accessible_default_action();
        settle();
        let confirm = window
            .root_element()
            .query_descendants()
            .match_predicate(|element| {
                element.accessible_label().is_some_and(|label| {
                    label == "Confirmer la suppression" || label == "Confirm deletion"
                })
            })
            .find_first()
            .expect("confirmation did not reopen");
        let position = slint::LogicalPosition::new(
            confirm.absolute_position().x + confirm.size().width / 2.0,
            confirm.absolute_position().y + confirm.size().height / 2.0,
        );
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerPressed { position, button });
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button });
        settle();
        assert_eq!(removed.borrow().as_slice(), &["meeting-0"]);
    }
}
