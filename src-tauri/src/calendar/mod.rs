//! Read-only macOS Calendar (EventKit) access.
//!
//! Everything goes through the local EventKit store, so any account synced
//! into Calendar.app (iCloud, Google, Outlook/Exchange, CalDAV) is visible
//! with zero cloud connectors. The app never writes to the calendar.
//!
//! Threading: `Retained<EKEventStore>` is `!Send`, so no EK object ever
//! crosses a thread boundary. Each public function allocates a fresh store,
//! does all EventKit work, and returns plain Rust DTOs. Callers invoke these
//! via `spawn_blocking` (they can block on TCC prompts or cross-process
//! calendar queries).

pub mod scheduler;

use chrono::{DateTime, Duration as ChronoDuration, Local, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::permissions::PermState;
use crate::transcript::MeetingParticipant;

/// One calendar as shown in the settings picker.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CalendarInfo {
    pub id: String,
    pub title: String,
    /// Account the calendar belongs to (e.g. "iCloud", "Google"), for grouping.
    pub source_title: Option<String>,
}

/// One occurrence of a calendar event (recurring events arrive pre-expanded).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CalendarEvent {
    /// EKEvent identifier — shared by all occurrences of a recurring event,
    /// so dedup keys must combine it with `start`.
    pub id: String,
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub calendar_id: String,
    pub calendar_title: String,
    pub participants: Vec<MeetingParticipant>,
    pub location: Option<String>,
    /// The event URL — for video meetings this is usually the conference link.
    pub url: Option<String>,
    /// The invitation body (EKCalendarItem notes). Mined for session-scoped
    /// transcription hints when a meeting starts from this event.
    pub description: Option<String>,
}

/// Today's events plus the permission state, so the UI can render the
/// no-permission case without string-matching errors.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct TodayCalendar {
    pub permission: PermState,
    pub events: Vec<CalendarEvent>,
}

/// Local-midnight to next local-midnight around `now`, as epoch seconds.
/// "Today" means the user's wall-clock day regardless of event timezones.
/// DST-ambiguous midnights resolve to the earliest valid instant.
fn local_day_bounds(now: DateTime<Local>) -> (i64, i64) {
    let day = now.date_naive();
    let start_naive = day.and_hms_opt(0, 0, 0).unwrap_or_else(|| {
        // 00:00:00 is always a valid time-of-day; keep a non-panicking fallback.
        now.naive_local()
    });
    let end_naive = start_naive + ChronoDuration::days(1);
    let resolve = |naive| match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => dt,
        chrono::LocalResult::Ambiguous(a, _) => a,
        // Skipped by a DST jump: shift forward an hour.
        chrono::LocalResult::None => {
            Local.from_utc_datetime(&(naive - ChronoDuration::hours(1)))
        }
    };
    (resolve(start_naive).timestamp(), resolve(end_naive).timestamp())
}

/// Strip a `mailto:` scheme from a participant URL, keeping only plausible
/// email addresses.
fn email_from_participant_url(url: &str) -> Option<String> {
    let candidate = url.strip_prefix("mailto:").unwrap_or(url);
    if candidate.contains('@') && !candidate.contains('/') {
        Some(candidate.to_string())
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use objc2::runtime::Bool;
    use objc2_event_kit::{
        EKAuthorizationStatus, EKEntityType, EKEvent, EKEventStore, EKParticipant,
        EKParticipantType,
    };
    use objc2_foundation::{NSDate, NSError};

    fn map_status(status: EKAuthorizationStatus) -> PermState {
        match status {
            EKAuthorizationStatus::NotDetermined => PermState::Unknown,
            // The deprecated pre-macOS-14 `Authorized` shares this value.
            EKAuthorizationStatus::FullAccess => PermState::Granted,
            // WriteOnly cannot read events, which is all we do.
            _ => PermState::Denied,
        }
    }

    pub fn authorization_state() -> PermState {
        map_status(unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) })
    }

    fn open_calendar_settings() {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Calendars")
            .spawn();
    }

    /// Trigger the TCC prompt (first time) or open System Settings (after a
    /// deny — macOS never re-prompts). Blocks until the user answers the
    /// dialog, so call from `spawn_blocking`.
    pub fn request_access() -> PermState {
        match authorization_state() {
            PermState::Granted => return PermState::Granted,
            PermState::Denied => {
                open_calendar_settings();
                return PermState::Denied;
            }
            _ => {}
        }

        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        crate::platform::with_autorelease_pool(|| {
            let store = unsafe { EKEventStore::new() };
            let block = block2::RcBlock::new(move |granted: Bool, _error: *mut NSError| {
                let _ = tx.send(granted.as_bool());
            });
            let handler = &*block as *const block2::DynBlock<dyn Fn(Bool, *mut NSError)>
                as *mut block2::DynBlock<dyn Fn(Bool, *mut NSError)>;
            let macos14 = objc2_foundation::NSProcessInfo::processInfo()
                .isOperatingSystemAtLeastVersion(objc2_foundation::NSOperatingSystemVersion {
                    majorVersion: 14,
                    minorVersion: 0,
                    patchVersion: 0,
                });
            unsafe {
                if macos14 {
                    store.requestFullAccessToEventsWithCompletion(handler);
                } else {
                    #[allow(deprecated)] // the only API available on macOS 13
                    store.requestAccessToEntityType_completion(EKEntityType::Event, handler);
                }
            }
            // The completion fires on an arbitrary queue after the user answers
            // the dialog; only the bool crosses back. Generous timeout: the
            // user may leave the dialog on screen.
            match rx.recv_timeout(std::time::Duration::from_secs(300)) {
                Ok(true) => PermState::Granted,
                _ => PermState::Denied,
            }
        })
    }

    pub fn list_calendars() -> Result<Vec<CalendarInfo>, String> {
        if authorization_state() != PermState::Granted {
            return Err("Calendar access not granted".to_string());
        }
        crate::platform::with_autorelease_pool(|| {
            let store = unsafe { EKEventStore::new() };
            let calendars = unsafe { store.calendarsForEntityType(EKEntityType::Event) };
            let mut out = Vec::with_capacity(calendars.len());
            for calendar in calendars.iter() {
                let source_title =
                    unsafe { calendar.source() }.map(|s| unsafe { s.title() }.to_string());
                out.push(CalendarInfo {
                    id: unsafe { calendar.calendarIdentifier() }.to_string(),
                    title: unsafe { calendar.title() }.to_string(),
                    source_title,
                });
            }
            out.sort_by(|a, b| {
                (a.source_title.as_deref(), a.title.as_str())
                    .cmp(&(b.source_title.as_deref(), b.title.as_str()))
            });
            Ok(out)
        })
    }

    fn convert_participant(participant: &EKParticipant, is_organizer: bool) -> MeetingParticipant {
        let url = unsafe { participant.URL().absoluteString() }
            .map(|s| s.to_string())
            .unwrap_or_default();
        let email = email_from_participant_url(&url);
        let name = unsafe { participant.name() }
            .map(|n| n.to_string())
            .filter(|n| !n.trim().is_empty())
            .or_else(|| email.clone())
            .unwrap_or_else(|| "Unknown".to_string());
        MeetingParticipant {
            name,
            email,
            is_organizer,
            is_current_user: unsafe { participant.isCurrentUser() },
        }
    }

    fn convert_event(event: &EKEvent) -> Option<CalendarEvent> {
        if unsafe { event.isAllDay() } {
            return None;
        }
        let title = unsafe { event.title() }.to_string();
        if title.trim().is_empty() {
            return None;
        }
        let calendar = unsafe { event.calendar() }?;
        let start = unsafe { event.startDate().timeIntervalSince1970() } as i64;
        let end = unsafe { event.endDate().timeIntervalSince1970() } as i64;

        // People and groups only; rooms/resources are noise for a transcript.
        let mut participants: Vec<MeetingParticipant> = Vec::new();
        if let Some(attendees) = unsafe { event.attendees() } {
            for attendee in attendees.iter() {
                let kind = unsafe { attendee.participantType() };
                if kind == EKParticipantType::Room || kind == EKParticipantType::Resource {
                    continue;
                }
                participants.push(convert_participant(&attendee, false));
            }
        }
        // The organizer is frequently absent from `attendees`; match by URL,
        // otherwise prepend as an extra participant.
        if let Some(organizer) = unsafe { event.organizer() } {
            let organizer_url = unsafe { organizer.URL().absoluteString() }
                .map(|s| s.to_string())
                .unwrap_or_default();
            let organizer_email = email_from_participant_url(&organizer_url);
            let matched = participants.iter_mut().find(|p| {
                p.email.is_some() && p.email == organizer_email
            });
            match matched {
                Some(p) => p.is_organizer = true,
                None => participants.insert(0, convert_participant(&organizer, true)),
            }
        }

        Some(CalendarEvent {
            id: unsafe { event.eventIdentifier() }
                .map(|s| s.to_string())
                .unwrap_or_default(),
            title,
            start: DateTime::from_timestamp(start, 0)?,
            end: DateTime::from_timestamp(end, 0)?,
            calendar_id: unsafe { calendar.calendarIdentifier() }.to_string(),
            calendar_title: unsafe { calendar.title() }.to_string(),
            participants,
            location: unsafe { event.location() }
                .map(|l| l.to_string())
                .filter(|l| !l.trim().is_empty()),
            url: unsafe { event.URL() }
                .and_then(|u| u.absoluteString().map(|s| s.to_string())),
            description: unsafe { event.notes() }
                .map(|n| n.to_string())
                .filter(|n| !n.trim().is_empty()),
        })
    }

    /// Timed events of the local day, sorted by start. `selected_calendar_ids`
    /// empty means all calendars. Recurring events arrive expanded into
    /// occurrences (that's what `eventsMatchingPredicate` does).
    pub fn fetch_todays_events(
        selected_calendar_ids: &[String],
    ) -> Result<Vec<CalendarEvent>, String> {
        if authorization_state() != PermState::Granted {
            return Err("Calendar access not granted".to_string());
        }
        let (day_start, day_end) = local_day_bounds(Local::now());
        crate::platform::with_autorelease_pool(|| {
            let store = unsafe { EKEventStore::new() };
            let start = NSDate::dateWithTimeIntervalSince1970(day_start as f64);
            let end = NSDate::dateWithTimeIntervalSince1970(day_end as f64);
            let predicate = unsafe {
                store.predicateForEventsWithStartDate_endDate_calendars(&start, &end, None)
            };
            let events = unsafe { store.eventsMatchingPredicate(&predicate) };
            let mut out: Vec<CalendarEvent> = events
                .iter()
                .filter_map(|e| convert_event(&e))
                .filter(|e| {
                    selected_calendar_ids.is_empty()
                        || selected_calendar_ids.contains(&e.calendar_id)
                })
                .collect();
            out.sort_by_key(|e| e.start);
            Ok(out)
        })
    }
}

#[cfg(target_os = "macos")]
pub use macos::{authorization_state, fetch_todays_events, list_calendars, request_access};

#[cfg(not(target_os = "macos"))]
pub fn authorization_state() -> PermState {
    PermState::Unsupported
}

#[cfg(not(target_os = "macos"))]
pub fn request_access() -> PermState {
    PermState::Unsupported
}

#[cfg(not(target_os = "macos"))]
pub fn list_calendars() -> Result<Vec<CalendarInfo>, String> {
    Ok(Vec::new())
}

#[cfg(not(target_os = "macos"))]
pub fn fetch_todays_events(
    _selected_calendar_ids: &[String],
) -> Result<Vec<CalendarEvent>, String> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Timelike};

    #[test]
    fn day_bounds_cover_exactly_one_day() {
        let now = Local.with_ymd_and_hms(2026, 7, 6, 15, 30, 0).unwrap();
        let (start, end) = local_day_bounds(now);
        assert_eq!(end - start, 86_400);
        let start_local = Local.timestamp_opt(start, 0).unwrap();
        assert_eq!(start_local.hour(), 0);
        assert_eq!(start_local.minute(), 0);
        assert_eq!(
            (start_local.year(), start_local.month(), start_local.day()),
            (2026, 7, 6)
        );
        assert!(now.timestamp() >= start && now.timestamp() < end);
    }

    #[test]
    fn email_extraction_from_mailto() {
        assert_eq!(
            email_from_participant_url("mailto:alice@corp.com"),
            Some("alice@corp.com".to_string())
        );
        assert_eq!(
            email_from_participant_url("alice@corp.com"),
            Some("alice@corp.com".to_string())
        );
        assert_eq!(email_from_participant_url("urn:uuid:1234"), None);
        assert_eq!(
            email_from_participant_url("https://example.com/user@host"),
            None
        );
    }
}
