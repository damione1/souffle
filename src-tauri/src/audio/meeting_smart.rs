//! Pure decision logic for smart meeting start/stop nudges.
//!
//! Coalesces calendar, process, and audio-activity signals into a single
//! start suggestion (process > audio > calendar). Strong end signals from
//! meeting-detect are classified here; the coordinator emits app events.

use std::time::{Duration, Instant};

use super::meeting_detect::{MeetingDetectSignal, MicCapturingApp};

/// Cooldown between start nudges while idle (OpenWhispr-style coalescence).
pub const START_NUDGE_COOLDOWN: Duration = Duration::from_millis(2500);

/// Grace period after a strong end nudge before auto-stop.
pub const END_NUDGE_AUTOSTOP: Duration = Duration::from_secs(10);

/// Re-signal interval while a strong end condition persists.
pub const END_NUDGE_RESIGNAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartNudgeSource {
    Process,
    AudioActivity,
    Calendar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartNudgeDecision {
    pub source: StartNudgeSource,
    /// Banner/notification title: calendar event name, app label, or fallback.
    pub title: String,
    pub app_label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrongEndReason {
    AppTerminated,
    KnownAppMicStopped,
    MicInactive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndNudgeDecision {
    pub reason: StrongEndReason,
    pub app_label: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct StartNudgeState {
    pub pending_process_label: Option<String>,
    pub pending_audio_active: bool,
}

#[derive(Debug, Clone, Default)]
pub struct EndNudgeMonitor {
    dismissed: bool,
    signaled_at: Option<Instant>,
    last_emit_at: Option<Instant>,
}

impl EndNudgeMonitor {
    pub fn note_strong_end(&mut self, now: Instant) -> bool {
        if self.dismissed {
            return false;
        }
        let first = self.signaled_at.is_none();
        let due = match self.last_emit_at {
            None => true,
            Some(last) => now.saturating_duration_since(last) >= END_NUDGE_RESIGNAL,
        };
        if !due {
            return false;
        }
        if first {
            self.signaled_at = Some(now);
        }
        self.last_emit_at = Some(now);
        true
    }

    pub fn dismiss(&mut self) {
        self.dismissed = true;
        self.signaled_at = None;
        self.last_emit_at = None;
    }

    pub fn rearm(&mut self) {
        self.dismissed = false;
        self.signaled_at = None;
        self.last_emit_at = None;
    }

    pub fn autostop_due(&self, now: Instant) -> bool {
        let Some(started) = self.signaled_at else {
            return false;
        };
        if self.dismissed {
            return false;
        }
        now.saturating_duration_since(started) >= END_NUDGE_AUTOSTOP
    }
}

/// Update pending start hints from a meeting-detect signal.
pub fn note_detect_signal(state: &mut StartNudgeState, signal: &MeetingDetectSignal) {
    match signal {
        MeetingDetectSignal::MicStarted(apps) => {
            if let Some(label) = primary_app_label(apps) {
                state.pending_process_label = Some(label);
            }
        }
        MeetingDetectSignal::MeetingAppLaunched(app) => {
            state.pending_process_label = Some(app.label.to_string());
        }
        MeetingDetectSignal::MicCaptureActive => {
            state.pending_audio_active = true;
        }
        MeetingDetectSignal::MicStopped(_) | MeetingDetectSignal::MeetingAppTerminated(_)
        | MeetingDetectSignal::MicCaptureInactive => {}
    }
}

/// Classify a meeting-detect signal as a strong end signal, if any.
pub fn strong_end_from_detect(signal: &MeetingDetectSignal) -> Option<EndNudgeDecision> {
    match signal {
        MeetingDetectSignal::MeetingAppTerminated(app) => Some(EndNudgeDecision {
            reason: StrongEndReason::AppTerminated,
            app_label: Some(app.label.to_string()),
        }),
        MeetingDetectSignal::MicStopped(apps) if !apps.is_empty() => Some(EndNudgeDecision {
            reason: StrongEndReason::KnownAppMicStopped,
            app_label: primary_app_label(apps),
        }),
        MeetingDetectSignal::MicCaptureInactive => Some(EndNudgeDecision {
            reason: StrongEndReason::MicInactive,
            app_label: None,
        }),
        _ => None,
    }
}

pub struct StartNudgeInput<'a> {
    pub state: &'a StartNudgeState,
    pub calendar_event_title: Option<&'a str>,
    pub recording: bool,
    pub smart_start_enabled: bool,
    pub last_nudge_at: Option<Instant>,
    pub now: Instant,
}

/// Decide whether to emit a coalesced start nudge.
pub fn evaluate_start_nudge(input: &StartNudgeInput<'_>) -> Option<StartNudgeDecision> {
    if input.recording || !input.smart_start_enabled {
        return None;
    }
    if let Some(last) = input.last_nudge_at
        && input.now.saturating_duration_since(last) < START_NUDGE_COOLDOWN
    {
        return None;
    }

    if let Some(label) = input.state.pending_process_label.as_deref() {
        let title = input
            .calendar_event_title
            .map(str::to_string)
            .unwrap_or_else(|| label.to_string());
        return Some(StartNudgeDecision {
            source: StartNudgeSource::Process,
            title,
            app_label: Some(label.to_string()),
        });
    }

    if input.state.pending_audio_active {
        let title = input
            .calendar_event_title
            .map(str::to_string)
            .unwrap_or_else(|| "Meeting".to_string());
        return Some(StartNudgeDecision {
            source: StartNudgeSource::AudioActivity,
            title,
            app_label: None,
        });
    }

    input.calendar_event_title.map(|title| StartNudgeDecision {
        source: StartNudgeSource::Calendar,
        title: title.to_string(),
        app_label: None,
    })
}

/// Clear pending hints consumed by an emitted start nudge.
pub fn consume_start_nudge(state: &mut StartNudgeState, source: StartNudgeSource) {
    match source {
        StartNudgeSource::Process => {
            state.pending_process_label = None;
            state.pending_audio_active = false;
        }
        StartNudgeSource::AudioActivity => {
            state.pending_audio_active = false;
        }
        StartNudgeSource::Calendar => {}
    }
}

fn primary_app_label(apps: &[MicCapturingApp]) -> Option<String> {
    apps.first().map(|app| app.label.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::meeting_detect::{MeetingAppRef, MicCapturingApp};

    fn mic_app(label: &'static str) -> MicCapturingApp {
        MicCapturingApp {
            pid: 1,
            bundle_id: "us.zoom.xos".into(),
            label,
        }
    }

    fn base_input<'a>(
        state: &'a StartNudgeState,
        now: Instant,
    ) -> StartNudgeInput<'a> {
        StartNudgeInput {
            state,
            calendar_event_title: None,
            recording: false,
            smart_start_enabled: true,
            last_nudge_at: None,
            now,
        }
    }

    #[test]
    fn process_beats_audio_and_calendar() {
        let mut state = StartNudgeState {
            pending_process_label: Some("Zoom".into()),
            pending_audio_active: true,
        };
        let input = StartNudgeInput {
            calendar_event_title: Some("Standup"),
            ..base_input(&state, Instant::now())
        };
        let decision = evaluate_start_nudge(&input).expect("nudge");
        assert_eq!(decision.source, StartNudgeSource::Process);
        assert_eq!(decision.title, "Standup");
        assert_eq!(decision.app_label.as_deref(), Some("Zoom"));

        consume_start_nudge(&mut state, StartNudgeSource::Process);
        assert!(state.pending_process_label.is_none());
        assert!(!state.pending_audio_active);
    }

    #[test]
    fn audio_beats_calendar_without_process() {
        let state = StartNudgeState {
            pending_audio_active: true,
            ..Default::default()
        };
        let input = StartNudgeInput {
            calendar_event_title: Some("Standup"),
            ..base_input(&state, Instant::now())
        };
        let decision = evaluate_start_nudge(&input).expect("nudge");
        assert_eq!(decision.source, StartNudgeSource::AudioActivity);
        assert_eq!(decision.title, "Standup");
    }

    #[test]
    fn calendar_only_when_no_other_signals() {
        let state = StartNudgeState::default();
        let input = StartNudgeInput {
            calendar_event_title: Some("Standup"),
            ..base_input(&state, Instant::now())
        };
        let decision = evaluate_start_nudge(&input).expect("nudge");
        assert_eq!(decision.source, StartNudgeSource::Calendar);
        assert_eq!(decision.title, "Standup");
    }

    #[test]
    fn suppress_while_recording() {
        let state = StartNudgeState {
            pending_process_label: Some("Zoom".into()),
            ..Default::default()
        };
        let input = StartNudgeInput {
            recording: true,
            ..base_input(&state, Instant::now())
        };
        assert!(evaluate_start_nudge(&input).is_none());
    }

    #[test]
    fn cooldown_blocks_back_to_back_nudges() {
        let state = StartNudgeState {
            pending_process_label: Some("Zoom".into()),
            ..Default::default()
        };
        let now = Instant::now();
        let input = StartNudgeInput {
            last_nudge_at: Some(now - Duration::from_millis(500)),
            ..base_input(&state, now)
        };
        assert!(evaluate_start_nudge(&input).is_none());
    }

    #[test]
    fn note_detect_mic_started_sets_process_label() {
        let mut state = StartNudgeState::default();
        note_detect_signal(
            &mut state,
            &MeetingDetectSignal::MicStarted(vec![mic_app("Zoom")]),
        );
        assert_eq!(state.pending_process_label.as_deref(), Some("Zoom"));
    }

    #[test]
    fn strong_end_from_app_terminated() {
        let decision = strong_end_from_detect(&MeetingDetectSignal::MeetingAppTerminated(
            MeetingAppRef {
                bundle_id: "us.zoom.xos".into(),
                label: "Zoom",
            },
        ))
        .expect("end");
        assert_eq!(decision.reason, StrongEndReason::AppTerminated);
        assert_eq!(decision.app_label.as_deref(), Some("Zoom"));
    }

    #[test]
    fn strong_end_from_mic_stopped_known_app() {
        let decision = strong_end_from_detect(&MeetingDetectSignal::MicStopped(vec![mic_app(
            "Teams",
        )]))
        .expect("end");
        assert_eq!(decision.reason, StrongEndReason::KnownAppMicStopped);
    }

    #[test]
    fn strong_end_from_mic_capture_inactive() {
        let decision =
            strong_end_from_detect(&MeetingDetectSignal::MicCaptureInactive).expect("end");
        assert_eq!(decision.reason, StrongEndReason::MicInactive);
    }

    #[test]
    fn end_monitor_autostop_after_grace() {
        let mut monitor = EndNudgeMonitor::default();
        let start = Instant::now();
        assert!(monitor.note_strong_end(start));
        assert!(!monitor.autostop_due(start + Duration::from_secs(5)));
        assert!(monitor.autostop_due(start + END_NUDGE_AUTOSTOP));
    }

    #[test]
    fn end_monitor_dismiss_rearms_cleanly() {
        let mut monitor = EndNudgeMonitor::default();
        let start = Instant::now();
        assert!(monitor.note_strong_end(start));
        monitor.dismiss();
        assert!(!monitor.note_strong_end(start + Duration::from_secs(1)));
        monitor.rearm();
        assert!(monitor.note_strong_end(start + Duration::from_secs(2)));
    }

    #[test]
    fn end_monitor_resignals_after_interval() {
        let mut monitor = EndNudgeMonitor::default();
        let start = Instant::now();
        assert!(monitor.note_strong_end(start));
        assert!(!monitor.note_strong_end(start + Duration::from_secs(5)));
        assert!(monitor.note_strong_end(start + END_NUDGE_RESIGNAL));
    }
}
