//! Background thread that turns meeting-detect signals into smart start/stop
//! app events and system notifications.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use chrono::Utc;
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;
use tauri_specta::Event;
use tracing::warn;

use crate::app_events::{
    MeetingEndNudge, MeetingEndNudgeReason, MeetingStartNudge, MeetingStartNudgeSource,
};
use crate::audio::meeting_detect::MeetingDetectSignal;
use crate::audio::meeting_smart::{
    EndNudgeMonitor, StartNudgeInput, StartNudgeSource, StartNudgeState, consume_start_nudge,
    evaluate_start_nudge, note_detect_signal, strong_end_from_detect,
};
use crate::audio::system_activity;
use crate::calendar::{self, CalendarEvent};
use crate::calendar::scheduler::{OccurrenceKey, due_autostart_nudges, prune_fired};
use crate::permissions::PermState;
use crate::settings::AppSettings;
use crate::state::AppState;

pub fn spawn(
    app: tauri::AppHandle,
    detect_rx: Receiver<MeetingDetectSignal>,
    end_monitor: std::sync::Arc<std::sync::Mutex<EndNudgeMonitor>>,
) {
    std::thread::Builder::new()
        .name("meeting-smart".into())
        .spawn(move || run(app, detect_rx, end_monitor))
        .expect("failed to spawn meeting-smart thread");
}

fn should_rearm_end_monitor(signal: &MeetingDetectSignal) -> bool {
    matches!(
        signal,
        MeetingDetectSignal::MicStarted(_)
            | MeetingDetectSignal::MicCaptureActive
            | MeetingDetectSignal::MeetingAppLaunched(_)
    )
}

fn run(
    app: tauri::AppHandle,
    detect_rx: Receiver<MeetingDetectSignal>,
    end_monitor: std::sync::Arc<std::sync::Mutex<EndNudgeMonitor>>,
) {
    let mut start_state = StartNudgeState::default();
    let mut last_start_nudge: Option<Instant> = None;
    let mut fired_autostart: HashSet<OccurrenceKey> = HashSet::new();
    let mut probe: Option<system_activity::SystemAudioProbe> = None;

    loop {
        match detect_rx.recv_timeout(Duration::from_secs(1)) {
            Ok(signal) => {
                note_detect_signal(&mut start_state, &signal);
                if should_rearm_end_monitor(&signal)
                    && let Ok(mut monitor) = end_monitor.lock()
                {
                    monitor.rearm();
                }
                if let Some(end) = strong_end_from_detect(&signal) {
                    handle_strong_end(&app, &end_monitor, end);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        tick_start_nudges(
            &app,
            &mut start_state,
            &mut last_start_nudge,
            &mut fired_autostart,
            &mut probe,
        );
        tick_end_autostop(&app, &end_monitor);
    }
}

fn recording_active(app: &tauri::AppHandle) -> bool {
    app.state::<AppState>()
        .current_machine_state()
        .map(|machine| machine.is_recording())
        .unwrap_or(false)
}

fn handle_strong_end(
    app: &tauri::AppHandle,
    monitor: &std::sync::Arc<std::sync::Mutex<EndNudgeMonitor>>,
    decision: crate::audio::meeting_smart::EndNudgeDecision,
) {
    if !recording_active(app) {
        return;
    }
    let settings = match AppSettings::load(&app.state::<AppState>().db) {
        Ok(settings) => settings,
        Err(e) => {
            warn!("Meeting smart: settings load failed: {e}");
            return;
        }
    };
    if !settings.meeting_smart_stop_enabled || !settings.meeting_autostop_enabled {
        return;
    }

    let now = Instant::now();
    let should_emit = monitor
        .lock()
        .map(|mut guard| guard.note_strong_end(now))
        .unwrap_or(false);
    if !should_emit {
        return;
    }

    let reason = match decision.reason {
        crate::audio::meeting_smart::StrongEndReason::AppTerminated => {
            MeetingEndNudgeReason::AppTerminated
        }
        crate::audio::meeting_smart::StrongEndReason::KnownAppMicStopped => {
            MeetingEndNudgeReason::KnownAppMicStopped
        }
        crate::audio::meeting_smart::StrongEndReason::MicInactive => {
            MeetingEndNudgeReason::MicInactive
        }
    };

    let payload = MeetingEndNudge {
        reason,
        app_label: decision.app_label.clone(),
    };
    notify_end_nudge(app, &payload, &settings.locale);
    if let Err(e) = payload.emit(app) {
        warn!("Meeting smart: end nudge emit failed: {e}");
    }
}

fn tick_end_autostop(
    app: &tauri::AppHandle,
    monitor: &std::sync::Arc<std::sync::Mutex<EndNudgeMonitor>>,
) {
    if !recording_active(app) {
        if let Ok(mut guard) = monitor.lock() {
            guard.rearm();
        }
        return;
    }
    let autostop = monitor
        .lock()
        .map(|guard| guard.autostop_due(Instant::now()))
        .unwrap_or(false);
    if autostop {
        if let Ok(mut guard) = monitor.lock() {
            guard.rearm();
        }
        let _ = crate::app_events::MeetingStopRequested.emit(app);
    }
}

fn tick_start_nudges(
    app: &tauri::AppHandle,
    start_state: &mut StartNudgeState,
    last_start_nudge: &mut Option<Instant>,
    fired_autostart: &mut HashSet<OccurrenceKey>,
    probe: &mut Option<system_activity::SystemAudioProbe>,
) {
    if recording_active(app) {
        *probe = None;
        return;
    }

    let state = app.state::<AppState>();
    let settings = match AppSettings::load(&state.db) {
        Ok(settings) => settings,
        Err(e) => {
            warn!("Meeting smart: settings load failed: {e}");
            return;
        }
    };

    let smart_start = settings.meeting_smart_start_enabled;
    let calendar_autostart = settings.calendar_autostart_enabled
        && settings.calendar_integration_enabled
        && calendar::authorization_state() == PermState::Granted;

    if !smart_start && !calendar_autostart {
        *probe = None;
        return;
    }

    let now_dt = Utc::now();
    prune_fired(fired_autostart, now_dt);

    let activity = state.system_audio_activity.clone();
    let calendar_event = calendar_autostart.then(|| next_calendar_autostart(app, fired_autostart)).flatten();
    let audio_active = activity.is_recently_active(system_activity::ACTIVITY_RECENCY);
    if audio_active {
        start_state.pending_audio_active = true;
    }

    let should_probe = calendar_event.is_some() && smart_start && !audio_active;
    if should_probe {
        if probe.is_none() {
            *probe = system_activity::SystemAudioProbe::start(activity);
        }
    } else {
        *probe = None;
    }

    let now = Instant::now();
    let input = StartNudgeInput {
        state: start_state,
        calendar_event_title: calendar_event.as_ref().map(|event| event.title.as_str()),
        recording: false,
        smart_start_enabled: smart_start || calendar_autostart,
        last_nudge_at: *last_start_nudge,
        now,
    };

    let Some(decision) = evaluate_start_nudge(&input) else {
        return;
    };

    // Calendar-only autostart is handled by the calendar scheduler when smart
    // start is off; when smart start is on, emit the coalesced nudge here.
    if decision.source == StartNudgeSource::Calendar && !smart_start {
        return;
    }

    emit_start_nudge(app, &decision, calendar_event.as_ref(), &settings.locale);
    *last_start_nudge = Some(now);
    consume_start_nudge(start_state, decision.source);

    if let Some(event) = calendar_event {
        fired_autostart.insert((event.id.clone(), event.start.timestamp()));
    }
}

fn next_calendar_autostart(
    app: &tauri::AppHandle,
    fired: &HashSet<OccurrenceKey>,
) -> Option<CalendarEvent> {
    let settings = AppSettings::load(&app.state::<AppState>().db).ok()?;
    let selected = settings.calendar_selected_ids.clone();
    let events = calendar::fetch_todays_events(&selected).ok()?;
    due_autostart_nudges(Utc::now(), &events, fired).into_iter().next()
}

fn emit_start_nudge(
    app: &tauri::AppHandle,
    decision: &crate::audio::meeting_smart::StartNudgeDecision,
    calendar_event: Option<&CalendarEvent>,
    locale: &str,
) {
    let source = match decision.source {
        StartNudgeSource::Process => MeetingStartNudgeSource::Process,
        StartNudgeSource::AudioActivity => MeetingStartNudgeSource::AudioActivity,
        StartNudgeSource::Calendar => MeetingStartNudgeSource::Calendar,
    };

    let payload = MeetingStartNudge {
        title: decision.title.clone(),
        source,
        app_label: decision.app_label.clone(),
        calendar_event: calendar_event.cloned(),
    };

    notify_start_nudge(app, &payload, locale);
    if let Err(e) = payload.emit(app) {
        warn!("Meeting smart: start nudge emit failed: {e}");
    }
}

fn notify_start_nudge(app: &tauri::AppHandle, nudge: &MeetingStartNudge, locale: &str) {
    let french = locale.starts_with("fr");
    let body = match nudge.source {
        MeetingStartNudgeSource::Process => {
            if let Some(label) = nudge.app_label.as_deref() {
                if french {
                    format!("{label} utilise le micro. Démarrer la transcription ?")
                } else {
                    format!("{label} is using the microphone. Start transcription?")
                }
            } else if french {
                "Une application de réunion utilise le micro. Démarrer la transcription ?"
                    .to_string()
            } else {
                "A meeting app is using the microphone. Start transcription?".to_string()
            }
        }
        MeetingStartNudgeSource::AudioActivity => {
            if french {
                "L'audio système est actif. Démarrer la transcription ?".to_string()
            } else {
                "System audio is active. Start transcription?".to_string()
            }
        }
        MeetingStartNudgeSource::Calendar => {
            if french {
                "La réunion a commencé. Démarrer la transcription ?".to_string()
            } else {
                "Your meeting started. Start transcription?".to_string()
            }
        }
    };

    if let Err(e) = app
        .notification()
        .builder()
        .title(&nudge.title)
        .body(&body)
        .show()
    {
        warn!("Meeting smart: start notification failed: {e}");
    }
}

fn notify_end_nudge(app: &tauri::AppHandle, nudge: &MeetingEndNudge, locale: &str) {
    let french = locale.starts_with("fr");
    let title = if french {
        "Réunion probablement terminée"
    } else {
        "Meeting seems to be over"
    };
    let body = match nudge.reason {
        MeetingEndNudgeReason::AppTerminated => {
            if let Some(label) = nudge.app_label.as_deref() {
                if french {
                    format!("{label} s'est fermé. Arrêt dans environ 10 s.")
                } else {
                    format!("{label} closed. Stopping in about 10 s.")
                }
            } else if french {
                "L'application de réunion s'est fermée. Arrêt dans environ 10 s.".to_string()
            } else {
                "The meeting app closed. Stopping in about 10 s.".to_string()
            }
        }
        MeetingEndNudgeReason::KnownAppMicStopped => {
            if let Some(label) = nudge.app_label.as_deref() {
                if french {
                    format!("{label} n'utilise plus le micro. Arrêt dans environ 10 s.")
                } else {
                    format!("{label} is no longer using the microphone. Stopping in about 10 s.")
                }
            } else if french {
                "Plus aucune application de réunion n'utilise le micro. Arrêt dans environ 10 s."
                    .to_string()
            } else {
                "No meeting app is using the microphone anymore. Stopping in about 10 s.".to_string()
            }
        }
        MeetingEndNudgeReason::MicInactive => {
            if french {
                "Le micro n'est plus utilisé. Arrêt dans environ 10 s.".to_string()
            } else {
                "The microphone is no longer in use. Stopping in about 10 s.".to_string()
            }
        }
    };

    if let Err(e) = app.notification().builder().title(title).body(&body).show() {
        warn!("Meeting smart: end notification failed: {e}");
    }
}
