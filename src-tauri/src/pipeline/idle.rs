//! Detects a meeting that has probably ended: either no speech for a
//! configured silence threshold, or a hard max-duration failsafe.
//!
//! Pure and unit-testable: all time math runs on `Instant`s passed in by the
//! caller, so tests never sleep for real.

use std::time::{Duration, Instant};

use crate::app_events::MeetingIdleReason;

/// How often a persisting silence (or max-duration) condition re-signals, so
/// a throttled/backgrounded webview still converges on the current state
/// instead of missing the one-shot signal.
const SILENCE_RESIGNAL_INTERVAL: Duration = Duration::from_secs(30);
const MAX_DURATION_RESIGNAL_INTERVAL: Duration = Duration::from_secs(60);

/// Session-start snapshot of the auto-stop settings. `None` on either field
/// disables that signal; a dictation session passes `None` for the whole
/// monitor since idle detection only makes sense for meetings.
#[derive(Debug, Clone, Copy)]
pub struct MeetingIdleConfig {
    pub silence_threshold: Option<Duration>,
    pub max_duration: Option<Duration>,
}

/// A detected idle condition, ready to be turned into an app event / OS
/// notification by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeetingIdleSignal {
    pub reason: MeetingIdleReason,
    pub idle_seconds: u64,
    pub threshold_seconds: u64,
    /// True only on the first signal for this idle episode: the caller should
    /// send an OS notification on `first == true` and skip it on re-signals.
    pub first: bool,
}

/// Tracks silence and total-duration to detect a meeting that is probably
/// over. Lives on the actor thread alongside `SessionHealth`; call
/// `note_segment` for every emitted segment with non-empty text and `tick`
/// on the same cadence as the health snapshot.
pub struct MeetingIdleMonitor {
    config: MeetingIdleConfig,
    started_at: Instant,
    last_activity: Instant,
    /// `Some` once silence has crossed the threshold for the current episode;
    /// cleared by `note_segment`. Tracks when the silence signal was last
    /// emitted, for the re-signal cadence.
    silence_signaled_at: Option<Instant>,
    /// Same idea for max-duration, which only ever fires once per session
    /// (there's no "re-arm": duration only grows).
    max_duration_signaled_at: Option<Instant>,
}

impl MeetingIdleMonitor {
    pub fn new(config: MeetingIdleConfig, started_at: Instant) -> Self {
        Self {
            config,
            started_at,
            last_activity: started_at,
            silence_signaled_at: None,
            max_duration_signaled_at: None,
        }
    }

    /// Record a segment with non-empty text: resets the silence clock and
    /// re-arms silence notifications for the next episode of silence.
    pub fn note_segment(&mut self, now: Instant) {
        self.last_activity = now;
        self.silence_signaled_at = None;
    }

    /// Check both conditions against `now`. Max-duration takes priority when
    /// both would fire in the same tick, since it stops immediately, but in
    /// practice a max-duration session will have long since been silent-idle
    /// too: the caller only needs one signal per tick.
    pub fn tick(&mut self, now: Instant) -> Option<MeetingIdleSignal> {
        if let Some(signal) = self.check_max_duration(now) {
            return Some(signal);
        }
        self.check_silence(now)
    }

    fn check_max_duration(&mut self, now: Instant) -> Option<MeetingIdleSignal> {
        let max_duration = self.config.max_duration?;
        let elapsed = now.saturating_duration_since(self.started_at);
        if elapsed < max_duration {
            return None;
        }

        let first = self.max_duration_signaled_at.is_none();
        let due = match self.max_duration_signaled_at {
            None => true,
            Some(last) => now.saturating_duration_since(last) >= MAX_DURATION_RESIGNAL_INTERVAL,
        };
        if !due {
            return None;
        }
        self.max_duration_signaled_at = Some(now);

        Some(MeetingIdleSignal {
            reason: MeetingIdleReason::MaxDuration,
            idle_seconds: elapsed.as_secs(),
            threshold_seconds: max_duration.as_secs(),
            first,
        })
    }

    fn check_silence(&mut self, now: Instant) -> Option<MeetingIdleSignal> {
        let threshold = self.config.silence_threshold?;
        let idle = now.saturating_duration_since(self.last_activity);
        if idle < threshold {
            return None;
        }

        let first = self.silence_signaled_at.is_none();
        let due = match self.silence_signaled_at {
            None => true,
            Some(last) => now.saturating_duration_since(last) >= SILENCE_RESIGNAL_INTERVAL,
        };
        if !due {
            return None;
        }
        self.silence_signaled_at = Some(now);

        Some(MeetingIdleSignal {
            reason: MeetingIdleReason::Silence,
            idle_seconds: idle.as_secs(),
            threshold_seconds: threshold.as_secs(),
            first,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(silence: Option<Duration>, max_duration: Option<Duration>) -> (MeetingIdleMonitor, Instant) {
        let start = Instant::now();
        let config = MeetingIdleConfig {
            silence_threshold: silence,
            max_duration,
        };
        (MeetingIdleMonitor::new(config, start), start)
    }

    #[test]
    fn no_signal_before_threshold() {
        let (mut m, start) = monitor(Some(Duration::from_secs(60)), None);
        assert_eq!(m.tick(start + Duration::from_secs(30)), None);
    }

    #[test]
    fn first_silence_crossing_signals_with_first_true() {
        let (mut m, start) = monitor(Some(Duration::from_secs(60)), None);
        let now = start + Duration::from_secs(61);
        let signal = m.tick(now).expect("should signal");
        assert_eq!(signal.reason, MeetingIdleReason::Silence);
        assert_eq!(signal.idle_seconds, 61);
        assert_eq!(signal.threshold_seconds, 60);
        assert!(signal.first);
    }

    #[test]
    fn resignals_every_30s_while_silence_persists() {
        let (mut m, start) = monitor(Some(Duration::from_secs(60)), None);
        let first_signal_at = start + Duration::from_secs(61);
        let first = m.tick(first_signal_at).expect("first signal");
        assert!(first.first);

        // Too soon: no re-signal yet.
        assert_eq!(m.tick(first_signal_at + Duration::from_secs(10)), None);

        // 30s after the first signal: re-signals, but first == false.
        let second = m
            .tick(first_signal_at + Duration::from_secs(30))
            .expect("resignal at 30s");
        assert!(!second.first);
        assert_eq!(second.reason, MeetingIdleReason::Silence);

        // Another 30s later: signals again.
        assert!(
            m.tick(first_signal_at + Duration::from_secs(60))
                .is_some()
        );
    }

    #[test]
    fn segment_rearms_silence_detection() {
        let (mut m, start) = monitor(Some(Duration::from_secs(60)), None);
        let now = start + Duration::from_secs(61);
        assert!(m.tick(now).is_some());

        // Activity resumes: silence clock resets and re-arms.
        m.note_segment(now + Duration::from_secs(1));

        // Not enough new silence yet.
        assert_eq!(m.tick(now + Duration::from_secs(30)), None);

        // Crosses threshold again from the new baseline; first == true again.
        let signal = m
            .tick(now + Duration::from_secs(1) + Duration::from_secs(61))
            .expect("should re-signal after new silence");
        assert!(signal.first);
    }

    #[test]
    fn max_duration_crossing_signals() {
        let (mut m, start) = monitor(None, Some(Duration::from_secs(3600)));
        assert_eq!(m.tick(start + Duration::from_secs(3599)), None);

        let signal = m
            .tick(start + Duration::from_secs(3601))
            .expect("should signal max duration");
        assert_eq!(signal.reason, MeetingIdleReason::MaxDuration);
        assert_eq!(signal.idle_seconds, 3601);
        assert_eq!(signal.threshold_seconds, 3600);
        assert!(signal.first);
    }

    #[test]
    fn max_duration_resignals_every_60s() {
        let (mut m, start) = monitor(None, Some(Duration::from_secs(3600)));
        let first_at = start + Duration::from_secs(3601);
        let first = m.tick(first_at).expect("first signal");
        assert!(first.first);

        assert_eq!(m.tick(first_at + Duration::from_secs(30)), None);

        let second = m
            .tick(first_at + Duration::from_secs(60))
            .expect("resignal at 60s");
        assert!(!second.first);
    }

    #[test]
    fn both_disabled_never_signals() {
        let (mut m, start) = monitor(None, None);
        assert_eq!(m.tick(start + Duration::from_secs(999_999)), None);
    }

    #[test]
    fn max_duration_takes_priority_over_silence_in_same_tick() {
        let (mut m, start) = monitor(Some(Duration::from_secs(60)), Some(Duration::from_secs(120)));
        let now = start + Duration::from_secs(121);
        let signal = m.tick(now).expect("should signal");
        assert_eq!(signal.reason, MeetingIdleReason::MaxDuration);
    }

    #[test]
    fn note_segment_before_any_silence_is_a_noop_for_threshold_math() {
        let (mut m, start) = monitor(Some(Duration::from_secs(60)), None);
        m.note_segment(start + Duration::from_secs(10));
        assert_eq!(m.tick(start + Duration::from_secs(40)), None);
        assert!(m.tick(start + Duration::from_secs(71)).is_some());
    }
}
