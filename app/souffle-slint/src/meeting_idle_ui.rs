//! Meeting auto-stop (SOU-294): the "meeting seems to be over" banner of the
//! recording view, and when the meeting stops by itself.
//!
//! The pipeline actor publishes its idle signal
//! (`souffle_lib::pipeline::live_meeting_idle`), re-signaled every 30 s while
//! silence lasts and every 60 s past the max-duration guard. The Tauri meeting
//! controller (`src/lib/features/meeting/controller.svelte.ts`,
//! `handleMeetingIdle`) acted on each signal; the recording timers poll it
//! here instead and do the same:
//!
//! - max duration: stop at once, whatever the user dismissed;
//! - silence: show the banner, and stop once the silence reaches the
//!   configured threshold plus [`SILENCE_AUTOSTOP_GRACE_SECONDS`];
//! - "Keep recording" hides the banner and cancels the stop for this silence
//!   episode only: speech resuming ends the episode, and the next one shows
//!   the banner again.
//!
//! A stop goes through the Stop button's own handler, so the meeting is
//! finalized, saved and opened exactly as if the user had clicked it.

use std::cell::Cell;

use crate::{MainWindow, MeetingIdleBanner, RecordingMode};
use souffle_lib::app_events::MeetingIdleReason;
use souffle_lib::pipeline::LiveMeetingIdle;

/// Extra silence tolerated after the banner first appears before the meeting
/// stops by itself, on top of the configured silence threshold (the Tauri
/// controller's `SILENCE_AUTOSTOP_GRACE_SECONDS`).
pub const SILENCE_AUTOSTOP_GRACE_SECONDS: u64 = 120;

/// What the live idle signal asks of the recording view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingIdleDecision {
    Hidden,
    /// The silence banner, with the silence so far in whole minutes.
    Warn {
        minutes: i32,
    },
    Stop {
        reason: MeetingIdleReason,
    },
}

pub fn meeting_idle_decision(
    idle: Option<LiveMeetingIdle>,
    dismissed_episode: Option<u64>,
) -> MeetingIdleDecision {
    let Some(idle) = idle else {
        return MeetingIdleDecision::Hidden;
    };
    match idle.reason {
        MeetingIdleReason::MaxDuration => MeetingIdleDecision::Stop {
            reason: MeetingIdleReason::MaxDuration,
        },
        MeetingIdleReason::Silence if dismissed_episode == Some(idle.episode) => {
            MeetingIdleDecision::Hidden
        }
        MeetingIdleReason::Silence
            if idle.idle_seconds
                >= idle
                    .threshold_seconds
                    .saturating_add(SILENCE_AUTOSTOP_GRACE_SECONDS) =>
        {
            MeetingIdleDecision::Stop {
                reason: MeetingIdleReason::Silence,
            }
        }
        MeetingIdleReason::Silence => MeetingIdleDecision::Warn {
            minutes: silence_minutes(idle.idle_seconds),
        },
    }
}

/// `Math.max(1, Math.round(idle_seconds / 60))`, as the Tauri banner said it.
fn silence_minutes(idle_seconds: u64) -> i32 {
    let minutes = idle_seconds.saturating_add(30) / 60;
    i32::try_from(minutes).unwrap_or(i32::MAX).max(1)
}

thread_local! {
    // The silence episode the user chose to keep recording through. UI
    // thread only: written by the banner's button, read by the poll.
    static DISMISSED_EPISODE: Cell<Option<u64>> = const { Cell::new(None) };
}

/// "Keep recording": hides the banner until speech resumes.
pub fn keep_recording(window: &MainWindow) {
    let episode = souffle_lib::pipeline::live_meeting_idle().map(|idle| idle.episode);
    DISMISSED_EPISODE.with(|dismissed| dismissed.set(episode));
    window.set_meeting_idle_banner(MeetingIdleBanner::Hidden);
}

/// The back-to-Idle path: no banner outlives its recording.
pub fn reset(window: &MainWindow) {
    DISMISSED_EPISODE.with(|dismissed| dismissed.set(None));
    window.set_meeting_idle_banner(MeetingIdleBanner::Hidden);
    window.set_meeting_idle_minutes(0);
}

/// One poll of the recording timers.
pub fn poll(window: &MainWindow) {
    if window.get_recording_mode() != RecordingMode::Meeting || window.get_recording_starting() {
        return;
    }
    // Latched until the stop settles and the view goes back to Idle: the
    // stop is already under way, whatever is said meanwhile.
    if window.get_meeting_idle_banner() == MeetingIdleBanner::MaxDuration {
        return;
    }
    let dismissed = DISMISSED_EPISODE.with(Cell::get);
    match meeting_idle_decision(souffle_lib::pipeline::live_meeting_idle(), dismissed) {
        MeetingIdleDecision::Hidden => {
            if window.get_meeting_idle_banner() != MeetingIdleBanner::Hidden {
                window.set_meeting_idle_banner(MeetingIdleBanner::Hidden);
            }
        }
        MeetingIdleDecision::Warn { minutes } => {
            if window.get_meeting_idle_minutes() != minutes {
                window.set_meeting_idle_minutes(minutes);
            }
            if window.get_meeting_idle_banner() != MeetingIdleBanner::Silence {
                window.set_meeting_idle_banner(MeetingIdleBanner::Silence);
            }
        }
        MeetingIdleDecision::Stop { reason } => {
            window.set_meeting_idle_banner(match reason {
                MeetingIdleReason::MaxDuration => MeetingIdleBanner::MaxDuration,
                MeetingIdleReason::Silence => MeetingIdleBanner::Hidden,
            });
            // The Stop button's handler ignores a stop already in flight, so
            // a signal seen again before the session ends is harmless.
            window.invoke_stop_requested();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle(reason: MeetingIdleReason, idle_seconds: u64, episode: u64) -> LiveMeetingIdle {
        LiveMeetingIdle {
            episode,
            reason,
            idle_seconds,
            threshold_seconds: 600,
        }
    }

    #[test]
    fn nothing_published_hides_the_banner() {
        assert_eq!(
            meeting_idle_decision(None, None),
            MeetingIdleDecision::Hidden
        );
        assert_eq!(
            meeting_idle_decision(None, Some(3)),
            MeetingIdleDecision::Hidden
        );
    }

    #[test]
    fn max_duration_stops_at_once_even_when_silence_was_dismissed() {
        let max = idle(MeetingIdleReason::MaxDuration, 14_400, 4);
        let stop = MeetingIdleDecision::Stop {
            reason: MeetingIdleReason::MaxDuration,
        };
        assert_eq!(meeting_idle_decision(Some(max), None), stop);
        assert_eq!(meeting_idle_decision(Some(max), Some(4)), stop);
    }

    #[test]
    fn silence_warns_before_the_grace_period_ends() {
        assert_eq!(
            meeting_idle_decision(Some(idle(MeetingIdleReason::Silence, 601, 1)), None),
            MeetingIdleDecision::Warn { minutes: 10 }
        );
        assert_eq!(
            meeting_idle_decision(Some(idle(MeetingIdleReason::Silence, 719, 1)), None),
            MeetingIdleDecision::Warn { minutes: 12 }
        );
    }

    #[test]
    fn silence_stops_once_it_reaches_threshold_plus_grace() {
        let stop = MeetingIdleDecision::Stop {
            reason: MeetingIdleReason::Silence,
        };
        assert_eq!(
            meeting_idle_decision(Some(idle(MeetingIdleReason::Silence, 720, 1)), None),
            stop
        );
        assert_eq!(
            meeting_idle_decision(Some(idle(MeetingIdleReason::Silence, 750, 1)), None),
            stop
        );
    }

    #[test]
    fn keep_recording_suppresses_only_the_dismissed_episode() {
        // Dismissed: neither the banner nor the stop, however long it lasts.
        assert_eq!(
            meeting_idle_decision(Some(idle(MeetingIdleReason::Silence, 631, 1)), Some(1)),
            MeetingIdleDecision::Hidden
        );
        assert_eq!(
            meeting_idle_decision(Some(idle(MeetingIdleReason::Silence, 900, 1)), Some(1)),
            MeetingIdleDecision::Hidden
        );
        // Speech resumed, then a new silence episode: shown again.
        assert_eq!(
            meeting_idle_decision(Some(idle(MeetingIdleReason::Silence, 601, 2)), Some(1)),
            MeetingIdleDecision::Warn { minutes: 10 }
        );
    }

    #[test]
    fn silence_minutes_round_like_the_tauri_banner_and_never_say_zero() {
        assert_eq!(silence_minutes(0), 1);
        assert_eq!(silence_minutes(29), 1);
        assert_eq!(silence_minutes(89), 1);
        assert_eq!(silence_minutes(90), 2);
        assert_eq!(silence_minutes(300), 5);
    }
}
