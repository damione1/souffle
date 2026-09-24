//! Port of features/timeline/controller.svelte.ts's pure functions
//! (toTimelineItems, groupByDay, day label formatting). Everything else in
//! that controller (search debounce, FTS result matching) is not wired yet -
//! see the "not wired yet" comments in main.rs.

use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use souffle_lib::calendar::CalendarEvent;
use souffle_lib::db::dictation::DictationEntry;
use souffle_lib::transcript::MeetingListItem;

use crate::{
    TimelineDay, TimelineDayGroup, TimelineEntry, TimelineFilter, TimelineKind, UpcomingEvent,
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
fn day_group(day: NaiveDate, today: NaiveDate, entries: Vec<TimelineEntry>) -> TimelineDayGroup {
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

    let today = Local::now().date_naive();
    groups
        .into_iter()
        .map(|(day, entries)| day_group(day, today, entries))
        .collect()
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

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
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
}
