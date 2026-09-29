//! Calendar reminder scheduler: a background task that watches today's
//! events and, shortly before one starts, sends a system notification.
//!
//! When an event is in progress and system audio is active but no recording
//! runs, a second at-event-time nudge fires (see
//! [`CalendarMeetingNudgeKind::Autostart`]).
//!
//! The fired-reminder set lives in memory only; restarting the app inside
//! the reminder window can re-fire one reminder for the same occurrence.
//! That rare duplicate is accepted over persisting scheduler state.
//!
//! SOU-191 note: before this ticket, each tick also emitted
//! [`crate::app_events::UpcomingMeeting`]/`TodayCalendarUpdated` for an
//! in-app one-click-start banner and a live-updating home list. Nothing in
//! the Slint shell ever listened for either (grepped project-wide, no
//! `.listen(` anywhere), so both were already inert; the system notification
//! below is the one part of this that was ever actually reaching the user,
//! and it still does. The banner/live-list UI itself is a pre-existing gap,
//! not something this ticket removed.
//!
//! SOU-285: the task no longer ticks. With the integration off it parks on
//! [`CALENDAR_CHANGED`] and does nothing until a settings save (or a wake
//! from system sleep) signals it. With it on, it sleeps until the next
//! moment something can become due (see [`next_wake`]), capped at
//! [`MAX_SLEEP`], and an EventKit change notification wakes it early.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::Notify;
use tracing::warn;

use crate::app_events::CalendarMeetingNudgeKind;
use crate::audio::mic_capture_probe;
use crate::calendar::{self, CalendarEvent};
use crate::permissions::PermState;
use crate::settings::{self, AppSettings};
use crate::state::AppState;

/// One occurrence of a (possibly recurring) event: the event identifier is
/// shared across occurrences, so the start timestamp disambiguates.
type OccurrenceKey = (String, i64);

/// How long after an event starts the auto-start nudge remains eligible.
const AUTOSTART_WINDOW_MINUTES: u32 = 10;

/// Longest sleep between two passes while the integration is on. A safety
/// net for anything no notification reports (a wall-clock change, a missed
/// EventKit notification).
const MAX_SLEEP: chrono::Duration = chrono::Duration::minutes(15);

/// While an event sits in its auto-start window, the mic-capture probe is
/// re-read at this cadence (the old fixed tick), since nothing notifies when
/// a meeting app opens the microphone.
const AUTOSTART_RECHECK: chrono::Duration = chrono::Duration::seconds(60);

/// Retry delay after a failed settings read or event fetch.
const RETRY_AFTER_ERROR: Duration = Duration::from_secs(60);

/// Wakes the scheduler: calendar settings saved, EventKit store changed, or
/// the system woke from sleep. `notify_one` keeps a permit when the task is
/// busy, so a signal raised mid-pass is not lost.
static CALENDAR_CHANGED: Notify = Notify::const_new();

/// Ask the scheduler to re-read its settings and re-fetch now.
pub fn wake() {
    CALENDAR_CHANGED.notify_one();
}

pub fn spawn(state: Arc<AppState>) {
    crate::async_runtime::spawn(run(state));
}

/// Sleep for `duration`, or less if [`wake`] is called.
async fn sleep_or_wake(duration: Duration) {
    tokio::select! {
        _ = tokio::time::sleep(duration) => {}
        _ = CALENDAR_CHANGED.notified() => {}
    }
}

async fn run(state: Arc<AppState>) {
    let mut fired_reminders: HashSet<OccurrenceKey> = HashSet::new();
    let mut fired_autostart: HashSet<OccurrenceKey> = HashSet::new();

    loop {
        // One key first: a disabled integration must not cost a full
        // settings read (let alone the migrating `AppSettings::load`, which
        // enumerates CoreAudio devices).
        match settings::calendar_integration_enabled(&state.db) {
            Ok(true) => {}
            Ok(false) => {
                CALENDAR_CHANGED.notified().await;
                continue;
            }
            Err(e) => {
                warn!("Calendar scheduler: settings read failed: {e}");
                sleep_or_wake(RETRY_AFTER_ERROR).await;
                continue;
            }
        }
        // Revoked mid-session (or not granted yet): go quiet until a
        // settings save or the safety cap, instead of erroring every pass.
        if calendar::authorization_state() != PermState::Granted {
            sleep_or_wake(MAX_SLEEP.to_std().unwrap_or(RETRY_AFTER_ERROR)).await;
            continue;
        }
        calendar::observe_store_changes(wake);

        let settings = match AppSettings::load_read_only(&state.db) {
            Ok(settings) => settings,
            Err(e) => {
                warn!("Calendar scheduler: settings load failed: {e}");
                sleep_or_wake(RETRY_AFTER_ERROR).await;
                continue;
            }
        };

        let recording = state
            .current_machine_state()
            .map(|machine| machine.is_recording())
            .unwrap_or(false);

        let selected = settings.calendar_selected_ids.clone();
        let events = match crate::async_runtime::spawn_blocking(move || {
            calendar::fetch_todays_events(&selected)
        })
        .await
        {
            Ok(Ok(events)) => events,
            Ok(Err(e)) => {
                warn!("Calendar scheduler: event fetch failed: {e}");
                sleep_or_wake(RETRY_AFTER_ERROR).await;
                continue;
            }
            Err(e) => {
                warn!("Calendar scheduler: fetch task failed: {e}");
                sleep_or_wake(RETRY_AFTER_ERROR).await;
                continue;
            }
        };

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
                &event,
                &settings.locale,
                CalendarMeetingNudgeKind::Reminder,
                starts_in_seconds,
            );
        }

        let due_autostart = if recording || !settings.calendar_autostart_enabled {
            Vec::new()
        } else {
            due_autostart_nudges(now, &events, &fired_autostart)
        };
        // The mic-capture read only happens with a nudge already pending, so
        // an ordinary event costs nothing beyond the pass itself.
        if !due_autostart.is_empty() && meeting_app_is_capturing_mic().await {
            for event in due_autostart {
                fired_autostart.insert((event.id.clone(), event.start.timestamp()));
                notify(
                    &event,
                    &settings.locale,
                    CalendarMeetingNudgeKind::Autostart,
                    0,
                );
            }
        }

        let now = Utc::now();
        let deadline = next_wake(
            now,
            &events,
            WakeInputs {
                reminder_minutes: settings.calendar_reminder_minutes,
                autostart_enabled: settings.calendar_autostart_enabled,
                day_end: calendar::local_day_end(),
                fired_reminders: &fired_reminders,
                fired_autostart: &fired_autostart,
            },
        );
        sleep_or_wake(duration_until(now, deadline)).await;
    }
}

/// What [`next_wake`] needs beyond the clock and the fetched events.
struct WakeInputs<'a> {
    reminder_minutes: u32,
    autostart_enabled: bool,
    /// Next local midnight: "today's events" changes meaning there.
    day_end: DateTime<Utc>,
    fired_reminders: &'a HashSet<OccurrenceKey>,
    fired_autostart: &'a HashSet<OccurrenceKey>,
}

/// The earliest instant at which a pass could have something to do: an
/// unfired reminder window opening, an event starting (auto-start), a
/// re-check of an event already in its auto-start window, local midnight,
/// or the [`MAX_SLEEP`] cap, whichever comes first.
fn next_wake(
    now: DateTime<Utc>,
    events: &[CalendarEvent],
    inputs: WakeInputs<'_>,
) -> DateTime<Utc> {
    let reminder_lead = chrono::Duration::minutes(i64::from(inputs.reminder_minutes));
    let autostart_window = chrono::Duration::minutes(i64::from(AUTOSTART_WINDOW_MINUTES));
    let mut deadline = (now + MAX_SLEEP).min(inputs.day_end);

    for event in events {
        let key = (event.id.clone(), event.start.timestamp());
        if event.start > now && !inputs.fired_reminders.contains(&key) {
            let reminder_at = event.start - reminder_lead;
            if reminder_at > now {
                deadline = deadline.min(reminder_at);
            }
        }
        if inputs.autostart_enabled && !inputs.fired_autostart.contains(&key) {
            if event.start > now {
                deadline = deadline.min(event.start);
            } else if now - event.start <= autostart_window && now < event.end {
                deadline = deadline.min(now + AUTOSTART_RECHECK);
            }
        }
    }
    deadline
}

/// `deadline - now` as a std duration, zero when already past.
fn duration_until(now: DateTime<Utc>, deadline: DateTime<Utc>) -> Duration {
    (deadline - now).to_std().unwrap_or(Duration::ZERO)
}

/// Whether anything is capturing the microphone right now. Runs on
/// a blocking worker: the CoreAudio property reads are cheap but they talk to
/// coreaudiod, which can stall.
async fn meeting_app_is_capturing_mic() -> bool {
    match crate::async_runtime::spawn_blocking(mic_capture_probe::mic_capture_in_progress).await {
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

/// System notification: informational only. Action buttons and click
/// callbacks are unreliable on macOS notifications either way, so this was
/// always the one reliable path (see the module doc for the in-app banner
/// this used to pair with).
fn notify(
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
    crate::native::notifications::notify(&event.title, &body);
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

    fn inputs<'a>(
        now: DateTime<Utc>,
        autostart_enabled: bool,
        fired_reminders: &'a HashSet<OccurrenceKey>,
        fired_autostart: &'a HashSet<OccurrenceKey>,
    ) -> WakeInputs<'a> {
        WakeInputs {
            reminder_minutes: 2,
            autostart_enabled,
            day_end: now + chrono::Duration::hours(12),
            fired_reminders,
            fired_autostart,
        }
    }

    #[test]
    fn next_wake_with_no_events_is_the_cap() {
        let now = Utc::now();
        let none = HashSet::new();
        assert_eq!(
            next_wake(now, &[], inputs(now, true, &none, &none)),
            now + MAX_SLEEP
        );
    }

    #[test]
    fn next_wake_is_capped_by_local_midnight() {
        let now = Utc::now();
        let none = HashSet::new();
        let mut wake_inputs = inputs(now, true, &none, &none);
        wake_inputs.day_end = now + chrono::Duration::minutes(3);
        assert_eq!(
            next_wake(now, &[], wake_inputs),
            now + chrono::Duration::minutes(3)
        );
    }

    /// SOU-285 AC2: an event in 3 min with a 2 min lead wakes the task when
    /// the reminder window opens, then at the start for auto-start.
    #[test]
    fn next_wake_targets_the_reminder_then_the_start() {
        let now = Utc::now();
        let none = HashSet::new();
        let meeting = event("a", now + chrono::Duration::minutes(3));
        assert_eq!(
            next_wake(
                now,
                std::slice::from_ref(&meeting),
                inputs(now, true, &none, &none)
            ),
            now + chrono::Duration::minutes(1)
        );

        let mut fired = HashSet::new();
        fired.insert(("a".to_string(), meeting.start.timestamp()));
        assert_eq!(
            next_wake(
                now,
                std::slice::from_ref(&meeting),
                inputs(now, true, &fired, &none)
            ),
            meeting.start
        );
        assert_eq!(
            next_wake(
                now,
                std::slice::from_ref(&meeting),
                inputs(now, false, &fired, &none)
            ),
            now + MAX_SLEEP,
            "auto-start off: nothing to do at the start"
        );
    }

    #[test]
    fn next_wake_rechecks_an_event_in_its_autostart_window() {
        let now = Utc::now();
        let none = HashSet::new();
        let started = event("a", now - chrono::Duration::minutes(2));
        assert_eq!(
            next_wake(
                now,
                std::slice::from_ref(&started),
                inputs(now, true, &none, &none)
            ),
            now + AUTOSTART_RECHECK
        );

        let mut fired = HashSet::new();
        fired.insert(("a".to_string(), started.start.timestamp()));
        assert_eq!(
            next_wake(
                now,
                std::slice::from_ref(&started),
                inputs(now, true, &none, &fired)
            ),
            now + MAX_SLEEP,
            "nudged already: no more re-checks"
        );

        let too_old = event("b", now - chrono::Duration::minutes(11));
        assert_eq!(
            next_wake(now, &[too_old], inputs(now, true, &none, &none)),
            now + MAX_SLEEP
        );
    }

    #[test]
    fn duration_until_a_past_deadline_is_zero() {
        let now = Utc::now();
        assert_eq!(
            duration_until(now, now - chrono::Duration::seconds(5)),
            Duration::ZERO
        );
        assert_eq!(
            duration_until(now, now + chrono::Duration::seconds(5)),
            Duration::from_secs(5)
        );
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
}
