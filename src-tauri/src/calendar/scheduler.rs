//! Calendar reminder scheduler: a background task that watches today's
//! events and, shortly before one starts, sends a system notification and
//! emits [`UpcomingMeeting`] so the frontend can offer a one-click start.
//!
//! When an event is in progress and system audio is active but no recording
//! runs, a second at-event-time nudge is emitted (see
//! [`CalendarMeetingNudgeKind::Autostart`]).
//!
//! The fired-reminder set lives in memory only; restarting the app inside
//! the reminder window can re-fire one reminder for the same occurrence.
//! That rare duplicate is accepted over persisting scheduler state.
//!
//! Each tick also diffs today's events and emits [`crate::app_events::TodayCalendarUpdated`]
//! so the home list stays current without depending on a webview timer.

use std::collections::HashSet;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;
use tauri_specta::Event;
use tokio::time::MissedTickBehavior;
use tracing::warn;

use crate::app_events::{CalendarMeetingNudgeKind, TodayCalendarUpdated, UpcomingMeeting};
use crate::audio::mic_capture_probe;
use crate::calendar::{self, CalendarEvent};
use crate::permissions::PermState;
use crate::settings::AppSettings;
use crate::state::AppState;

/// One occurrence of a (possibly recurring) event: the event identifier is
/// shared across occurrences, so the start timestamp disambiguates.
type OccurrenceKey = (String, i64);

/// How long after an event starts the auto-start nudge remains eligible.
const AUTOSTART_WINDOW_MINUTES: u32 = 10;

type TodayFingerprint = (PermState, Vec<(String, i64, i64, String)>);

pub fn spawn(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(run(app));
}

async fn run(app: tauri::AppHandle) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut fired_reminders: HashSet<OccurrenceKey> = HashSet::new();
    let mut fired_autostart: HashSet<OccurrenceKey> = HashSet::new();
    let mut last_today: Option<TodayFingerprint> = None;

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
            emit_today_if_changed(&app, &mut last_today, PermState::Unknown, &[]);
            continue;
        }
        // Revoked mid-session: go quiet instead of erroring every minute.
        let permission = calendar::authorization_state();
        if permission != PermState::Granted {
            emit_today_if_changed(&app, &mut last_today, permission, &[]);
            continue;
        }

        let recording = app
            .state::<AppState>()
            .current_machine_state()
            .map(|machine| machine.is_recording())
            .unwrap_or(false);

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

        emit_today_if_changed(&app, &mut last_today, PermState::Granted, &events);

        let now = Utc::now();
        prune_fired(&mut fired_reminders, now);
        prune_fired(&mut fired_autostart, now);

        for event in due_reminders(
            now,
            &events,
            settings.calendar_reminder_minutes,
            &fired_reminders,
        ) {
            fired_reminders.insert((event.id.clone(), event.start.timestamp()));
            let starts_in_seconds = (event.start - now).num_seconds().max(0) as u32;
            notify(
                &app,
                &event,
                &settings.locale,
                CalendarMeetingNudgeKind::Reminder,
                starts_in_seconds,
            );
            if let Err(e) = (UpcomingMeeting {
                event,
                starts_in_seconds,
                kind: CalendarMeetingNudgeKind::Reminder,
            })
            .emit(&app)
            {
                warn!("Calendar scheduler: emit failed: {e}");
            }
        }

        let due_autostart = if recording || !settings.calendar_autostart_enabled {
            Vec::new()
        } else {
            due_autostart_nudges(now, &events, &fired_autostart)
        };
        // The mic-capture read only happens with a nudge already pending, so
        // an ordinary event costs nothing beyond the tick itself.
        if !due_autostart.is_empty() && meeting_app_is_capturing_mic().await {
            for event in due_autostart {
                fired_autostart.insert((event.id.clone(), event.start.timestamp()));
                notify(
                    &app,
                    &event,
                    &settings.locale,
                    CalendarMeetingNudgeKind::Autostart,
                    0,
                );
                if let Err(e) = (UpcomingMeeting {
                    event,
                    starts_in_seconds: 0,
                    kind: CalendarMeetingNudgeKind::Autostart,
                })
                .emit(&app)
                {
                    warn!("Calendar scheduler: autostart emit failed: {e}");
                }
            }
        }
    }
}

/// Whether anything is capturing the microphone right now. Runs on
/// a blocking worker: the CoreAudio property reads are cheap but they talk to
/// coreaudiod, which can stall.
async fn meeting_app_is_capturing_mic() -> bool {
    match tauri::async_runtime::spawn_blocking(mic_capture_probe::mic_capture_in_progress).await {
        Ok(Some(capture)) => {
            tracing::debug!(?capture, "Calendar autostart: the mic is in use");
            true
        }
        Ok(None) => false,
        Err(e) => {
            warn!("Calendar scheduler: mic-capture probe failed: {e}");
            false
        }
    }
}

/// Events whose start lies within the reminder window and that have not
/// fired yet. Already-started events are excluded: a late reminder is noise.
pub fn due_reminders(
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

/// Events that have started recently, are still in progress, and have not
/// yet received an at-event-time auto-start nudge.
pub fn due_autostart_nudges(
    now: DateTime<Utc>,
    events: &[CalendarEvent],
    fired: &HashSet<OccurrenceKey>,
) -> Vec<CalendarEvent> {
    let window = chrono::Duration::minutes(i64::from(AUTOSTART_WINDOW_MINUTES));
    events
        .iter()
        .filter(|event| {
            let elapsed = now - event.start;
            elapsed >= chrono::Duration::zero()
                && elapsed <= window
                && now < event.end
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

fn today_fingerprint(permission: PermState, events: &[CalendarEvent]) -> TodayFingerprint {
    (
        permission,
        events
            .iter()
            .map(|event| {
                (
                    event.id.clone(),
                    event.start.timestamp(),
                    event.end.timestamp(),
                    event.title.clone(),
                )
            })
            .collect(),
    )
}

/// Push today's events to the frontend when the snapshot actually changed
/// (new invite, reschedule, midnight rollover). The home view cannot rely
/// on a webview timer surviving overnight in the background.
fn emit_today_if_changed(
    app: &tauri::AppHandle,
    last: &mut Option<TodayFingerprint>,
    permission: PermState,
    events: &[CalendarEvent],
) {
    let fingerprint = today_fingerprint(permission, events);
    if last.as_ref() == Some(&fingerprint) {
        return;
    }
    *last = Some(fingerprint);
    if let Err(e) = (TodayCalendarUpdated {
        permission,
        events: events.to_vec(),
    })
    .emit(app)
    {
        warn!("Calendar scheduler: today emit failed: {e}");
    }
}

/// System notification: informational only. Action buttons and click
/// callbacks are unreliable on macOS with the notification plugin, so the
/// actionable path is the in-app banner driven by [`UpcomingMeeting`].
fn notify(
    app: &tauri::AppHandle,
    event: &CalendarEvent,
    locale: &str,
    kind: CalendarMeetingNudgeKind,
    starts_in_seconds: u32,
) {
    let body = match kind {
        CalendarMeetingNudgeKind::Reminder => {
            let minutes = starts_in_seconds.div_ceil(60).max(1);
            if locale.starts_with("fr") {
                format!("Commence dans {minutes} min. Ouvrez Soufflé pour transcrire la réunion.")
            } else {
                format!("Starts in {minutes} min. Open Soufflé to transcribe the meeting.")
            }
        }
        CalendarMeetingNudgeKind::Autostart => {
            if locale.starts_with("fr") {
                "La réunion a commencé et l'audio système est actif. Démarrer l'enregistrement ?"
                    .to_string()
            } else {
                "Your meeting started and system audio is active. Start recording?".to_string()
            }
        }
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
        let beyond = event(
            "b",
            now + chrono::Duration::minutes(2) + chrono::Duration::seconds(1),
        );
        let due = due_reminders(now, &[at_boundary, beyond], 2, &fired);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, "a");
    }

    #[test]
    fn already_started_events_are_not_due_for_reminder() {
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
    fn autostart_nudges_only_after_start_within_window() {
        let now = Utc::now();
        let fired = HashSet::new();
        let just_started = event("a", now - chrono::Duration::minutes(1));
        let not_started = event("b", now + chrono::Duration::minutes(5));
        let too_old = event("c", now - chrono::Duration::minutes(11));
        let due = due_autostart_nudges(now, &[just_started, not_started, too_old], &fired);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, "a");
    }

    #[test]
    fn autostart_skips_fired_occurrences() {
        let now = Utc::now();
        let started = event("a", now - chrono::Duration::minutes(2));
        let mut fired = HashSet::new();
        fired.insert(("a".to_string(), started.start.timestamp()));
        let due = due_autostart_nudges(now, &[started], &fired);
        assert!(due.is_empty());
    }

    #[test]
    fn prune_drops_only_stale_keys() {
        let now = Utc::now();
        let mut fired = HashSet::new();
        fired.insert((
            "old".to_string(),
            (now - chrono::Duration::days(2)).timestamp(),
        ));
        fired.insert((
            "recent".to_string(),
            (now - chrono::Duration::hours(1)).timestamp(),
        ));
        prune_fired(&mut fired, now);
        assert_eq!(fired.len(), 1);
        assert!(fired.iter().any(|(id, _)| id == "recent"));
    }

    #[test]
    fn today_fingerprint_changes_on_new_or_retitled_event() {
        let now = Utc::now();
        let first = event("a", now);
        let retitled = CalendarEvent {
            title: "Renamed".to_string(),
            ..first.clone()
        };
        let granted = PermState::Granted;
        assert_eq!(
            today_fingerprint(granted, std::slice::from_ref(&first)),
            today_fingerprint(granted, std::slice::from_ref(&first))
        );
        assert_ne!(
            today_fingerprint(granted, std::slice::from_ref(&first)),
            today_fingerprint(granted, &[retitled])
        );
        assert_ne!(
            today_fingerprint(granted, &[first]),
            today_fingerprint(granted, &[])
        );
    }
}
