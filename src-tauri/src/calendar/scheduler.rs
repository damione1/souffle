//! Calendar reminder scheduler: a background task that watches today's
//! events and, shortly before one starts, sends a system notification and
//! emits [`UpcomingMeeting`] so the frontend can offer a one-click start.
//!
//! The fired-reminder set lives in memory only; restarting the app inside
//! the reminder window can re-fire one reminder for the same occurrence.
//! That rare duplicate is accepted over persisting scheduler state.

use std::collections::HashSet;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;
use tauri_specta::Event;
use tokio::time::MissedTickBehavior;
use tracing::warn;

use crate::app_events::UpcomingMeeting;
use crate::calendar::{self, CalendarEvent};
use crate::permissions::PermState;
use crate::settings::AppSettings;
use crate::state::AppState;

/// One occurrence of a (possibly recurring) event: the event identifier is
/// shared across occurrences, so the start timestamp disambiguates.
type OccurrenceKey = (String, i64);

pub fn spawn(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(run(app));
}

async fn run(app: tauri::AppHandle) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut fired: HashSet<OccurrenceKey> = HashSet::new();

    loop {
        interval.tick().await;

        // Settings are re-read every tick so toggling the integration (or
        // changing the lead time) needs no scheduler restart.
        let settings = {
            let state = app.state::<AppState>();
            match AppSettings::load(&state.db) {
                Ok(settings) => settings,
                Err(e) => {
                    warn!("Calendar scheduler: settings load failed: {e}");
                    continue;
                }
            }
        };
        if !settings.calendar_integration_enabled {
            continue;
        }
        // Revoked mid-session: go quiet instead of erroring every minute.
        if calendar::authorization_state() != PermState::Granted {
            continue;
        }

        let selected = settings.calendar_selected_ids.clone();
        let events = match tauri::async_runtime::spawn_blocking(move || {
            calendar::fetch_todays_events(&selected)
        })
        .await
        {
            Ok(Ok(events)) => events,
            Ok(Err(e)) => {
                warn!("Calendar scheduler: event fetch failed: {e}");
                continue;
            }
            Err(e) => {
                warn!("Calendar scheduler: fetch task failed: {e}");
                continue;
            }
        };

        let now = Utc::now();
        prune_fired(&mut fired, now);

        for event in due_reminders(now, &events, settings.calendar_reminder_minutes, &fired) {
            fired.insert((event.id.clone(), event.start.timestamp()));
            let starts_in_seconds = (event.start - now).num_seconds().max(0) as u32;
            notify(&app, &event, &settings.locale, starts_in_seconds);
            if let Err(e) = (UpcomingMeeting {
                event,
                starts_in_seconds,
            })
            .emit(&app)
            {
                warn!("Calendar scheduler: emit failed: {e}");
            }
        }
    }
}

/// Events whose start lies within the reminder window and that have not
/// fired yet. Already-started events are excluded: a late reminder is noise.
fn due_reminders(
    now: DateTime<Utc>,
    events: &[CalendarEvent],
    reminder_minutes: u32,
    fired: &HashSet<OccurrenceKey>,
) -> Vec<CalendarEvent> {
    let window = chrono::Duration::minutes(i64::from(reminder_minutes));
    events
        .iter()
        .filter(|event| {
            now < event.start
                && event.start - now <= window
                && !fired.contains(&(event.id.clone(), event.start.timestamp()))
        })
        .cloned()
        .collect()
}

/// Drop fired keys older than a day so the set stays bounded.
fn prune_fired(fired: &mut HashSet<OccurrenceKey>, now: DateTime<Utc>) {
    let cutoff = (now - chrono::Duration::days(1)).timestamp();
    fired.retain(|(_, start)| *start >= cutoff);
}

/// System notification: informational only. Action buttons and click
/// callbacks are unreliable on macOS with the notification plugin, so the
/// actionable path is the in-app banner driven by [`UpcomingMeeting`].
fn notify(app: &tauri::AppHandle, event: &CalendarEvent, locale: &str, starts_in_seconds: u32) {
    let minutes = starts_in_seconds.div_ceil(60).max(1);
    let body = if locale.starts_with("fr") {
        format!("Commence dans {minutes} min. Ouvrez Soufflé pour transcrire la réunion.")
    } else {
        format!("Starts in {minutes} min. Open Soufflé to transcribe the meeting.")
    };
    if let Err(e) = app
        .notification()
        .builder()
        .title(&event.title)
        .body(&body)
        .show()
    {
        warn!("Calendar scheduler: notification failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::MeetingParticipant;

    fn event(id: &str, start: DateTime<Utc>) -> CalendarEvent {
        CalendarEvent {
            id: id.to_string(),
            title: "Standup".to_string(),
            start,
            end: start + chrono::Duration::minutes(30),
            calendar_id: "cal-1".to_string(),
            calendar_title: "Work".to_string(),
            participants: Vec::<MeetingParticipant>::new(),
            location: None,
            url: None,
            description: None,
        }
    }

    #[test]
    fn due_exactly_at_window_boundary_and_not_before() {
        let now = Utc::now();
        let fired = HashSet::new();
        let at_boundary = event("a", now + chrono::Duration::minutes(2));
        let beyond = event("b", now + chrono::Duration::minutes(2) + chrono::Duration::seconds(1));
        let due = due_reminders(now, &[at_boundary, beyond], 2, &fired);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, "a");
    }

    #[test]
    fn already_started_events_are_not_due() {
        let now = Utc::now();
        let fired = HashSet::new();
        let started = event("a", now - chrono::Duration::seconds(1));
        let due = due_reminders(now, &[started], 2, &fired);
        assert!(due.is_empty());
    }

    #[test]
    fn fired_occurrences_do_not_refire_but_other_occurrences_do() {
        let now = Utc::now();
        let first = event("recurring", now + chrono::Duration::minutes(1));
        let second = event("recurring", now + chrono::Duration::minutes(2));
        let mut fired = HashSet::new();
        fired.insert(("recurring".to_string(), first.start.timestamp()));
        let due = due_reminders(now, &[first, second.clone()], 2, &fired);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].start, second.start);
    }

    #[test]
    fn prune_drops_only_stale_keys() {
        let now = Utc::now();
        let mut fired = HashSet::new();
        fired.insert(("old".to_string(), (now - chrono::Duration::days(2)).timestamp()));
        fired.insert(("recent".to_string(), (now - chrono::Duration::hours(1)).timestamp()));
        prune_fired(&mut fired, now);
        assert_eq!(fired.len(), 1);
        assert!(fired.iter().any(|(id, _)| id == "recent"));
    }
}
