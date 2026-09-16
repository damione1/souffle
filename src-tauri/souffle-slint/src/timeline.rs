//! Port of features/timeline/controller.svelte.ts's pure functions
//! (toTimelineItems, groupByDay, day label formatting). Everything else in
//! that controller (search debounce, FTS result matching) is not wired yet -
//! see the "not wired yet" comments in main.rs.

use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use souffle_lib::db::dictation::DictationEntry;
use souffle_lib::transcript::MeetingListItem;

use crate::{TimelineDayGroup, TimelineEntry, TimelineFilter, TimelineKind};

const WEEKDAYS: [&str; 7] = [
    "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche",
];
const MONTHS: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

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

fn day_label(day: NaiveDate) -> String {
    let today = Local::now().date_naive();
    if day == today {
        return "Aujourd'hui".to_string();
    }
    if day == today.pred_opt().unwrap_or(today) {
        return "Hier".to_string();
    }
    let weekday = WEEKDAYS[day.weekday().num_days_from_monday() as usize];
    let month = MONTHS[(day.month0()) as usize];
    format!("{weekday} {} {month}", day.day())
}

/// One merged, sorted (newest first) entry per dictation/meeting - mirrors
/// `toTimelineItems`. Returns entries paired with their sort/group key so
/// the caller doesn't have to re-parse the timestamp.
fn merge_entries(
    dictations: &[DictationEntry],
    meetings: &[MeetingListItem],
) -> Vec<(DateTime<Utc>, TimelineEntry)> {
    let mut entries: Vec<(DateTime<Utc>, TimelineEntry)> =
        Vec::with_capacity(dictations.len() + meetings.len());

    for entry in dictations {
        let Ok(at) = DateTime::parse_from_rfc3339(&entry.timestamp) else {
            continue;
        };
        let at = at.with_timezone(&Utc);
        entries.push((
            at,
            TimelineEntry {
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
            TimelineEntry {
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

    entries.sort_by(|a, b| b.0.cmp(&a.0));
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

    let mut groups: Vec<(NaiveDate, Vec<TimelineEntry>)> = Vec::new();
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

    groups
        .into_iter()
        .map(|(day, entries)| TimelineDayGroup {
            day_label: day_label(day).into(),
            entries: std::rc::Rc::new(slint::VecModel::from(entries)).into(),
        })
        .collect()
}
