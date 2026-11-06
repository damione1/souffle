//! Native compression projection shared by the app and synthetic QA fixture.
use crate::{CompactJobPhase, CompactLine, CompressionModelPhase, CompressionUi, MainWindow};
use slint::ComponentHandle;
use souffle_lib::{
    models::compression::ModelStatus,
    settings::{AppSettings, SettingsOptions},
    state::AppState,
    summary::compress::CompactJobStatus,
    transcript::MeetingTranscript,
};
use std::{rc::Rc, sync::Arc, time::Duration};

fn model_phase(status: &ModelStatus) -> CompressionModelPhase {
    match status {
        ModelStatus::NotDownloaded => CompressionModelPhase::NotDownloaded,
        ModelStatus::Downloading { .. } => CompressionModelPhase::Downloading,
        ModelStatus::Preparing => CompressionModelPhase::Preparing,
        ModelStatus::Ready { .. } => CompressionModelPhase::Ready,
        ModelStatus::Failed { .. } => CompressionModelPhase::Failed,
    }
}
fn job_phase(status: &CompactJobStatus) -> CompactJobPhase {
    match status {
        CompactJobStatus::Idle => CompactJobPhase::Idle,
        CompactJobStatus::WaitingForModel => CompactJobPhase::WaitingForModel,
        CompactJobStatus::Queued => CompactJobPhase::Queued,
        CompactJobStatus::Compressing { .. } => CompactJobPhase::Compressing,
        CompactJobStatus::Failed { .. } => CompactJobPhase::Failed,
        CompactJobStatus::SourceChanged => CompactJobPhase::SourceChanged,
        CompactJobStatus::InterruptedByRecording => CompactJobPhase::InterruptedByRecording,
    }
}
pub fn populate_settings(window: &MainWindow, settings: &AppSettings) {
    let ui = window.global::<CompressionUi>();
    let options = SettingsOptions::current();
    ui.set_summary_enabled(settings.summary_compression_enabled);
    ui.set_alongside_enabled(settings.compact_transcript_enabled);
    ui.set_rate((settings.summary_compression_rate * 100.).round() as i32);
    ui.set_rate_min((options.compression_rate_min * 100.).round() as i32);
    ui.set_rate_max((options.compression_rate_max * 100.).round() as i32);
}
fn preview(text: &str, expanded: bool) -> Vec<CompactLine> {
    text.lines()
        .take(if expanded { usize::MAX } else { 4 })
        .map(|line| {
            let timestamp = line
                .strip_prefix('[')
                .and_then(|s| s.split_once(']'))
                .and_then(|(time, _)| time.split_once(':'))
                .and_then(|(m, s)| Some(m.parse::<f32>().ok()? * 60. + s.parse::<f32>().ok()?));
            CompactLine {
                text: line.into(),
                seconds: timestamp.unwrap_or(0.),
                timed: timestamp.is_some(),
            }
        })
        .collect()
}
pub fn populate(window: &MainWindow, meeting: &MeetingTranscript) {
    let ui = window.global::<CompressionUi>();
    if ui.get_meeting_id().as_str() != meeting.id {
        ui.set_ui_generation(uuid::Uuid::new_v4().to_string().into());
        ui.set_expanded(false);
        ui.set_copied(false);
        ui.set_job_phase(CompactJobPhase::Idle);
        ui.set_job_error("".into());
    }
    ui.set_meeting_id(meeting.id.clone().into());
    ui.set_ongoing(meeting.ended_at.is_none());
    ui.set_has_compact(meeting.compact_transcript.is_some());
    if let Some(compact) = &meeting.compact_transcript {
        ui.set_stale(compact.is_stale);
        ui.set_generated_at(compact.generated_at.clone().into());
        ui.set_compact_rate((compact.rate * 100.).round() as i32);
        ui.set_approx_tokens(compact.approx_tokens.min(i32::MAX as usize) as i32);
        ui.set_lines(
            Rc::new(slint::VecModel::from(preview(
                &compact.text,
                ui.get_expanded(),
            )))
            .into(),
        );
    } else {
        ui.set_stale(false);
        ui.set_lines(Default::default());
    }
}
/// Keeps state across settings tabs; no model, DB or UI lock crosses inference.
pub fn install(window: &MainWindow, state: Arc<AppState>) -> slint::Timer {
    let weak = window.as_weak();
    let service = state.compression.clone();
    let poll_state = state.clone();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(300),
        move || {
            let Some(w) = weak.upgrade() else { return };
            let ui = w.global::<CompressionUi>();
            let status = service.model_status();
            ui.set_model_phase(model_phase(&status));
            ui.set_model_size(
                format!(
                    "{:.1} MB",
                    match status {
                        ModelStatus::Ready { bytes } => bytes,
                        ModelStatus::NotDownloaded
                        | ModelStatus::Downloading { .. }
                        | ModelStatus::Preparing
                        | ModelStatus::Failed { .. } =>
                            souffle_lib::models::compression::download_bytes(),
                    } as f64
                        / 1_000_000.
                )
                .into(),
            );
            if let ModelStatus::Downloading { downloaded, total } = &status {
                ui.set_download_progress(
                    format!(
                        "{:.1} / {:.1} MB",
                        *downloaded as f64 / 1_000_000.,
                        *total as f64 / 1_000_000.
                    )
                    .into(),
                );
            }
            if let ModelStatus::Failed { detail } = &status {
                ui.set_model_error(detail.clone().into());
            }
            if w.get_active_meeting_id() == ui.get_meeting_id() {
                let job = service.job(ui.get_meeting_id().as_str());
                let generation = job.as_ref().map(|job| job.generation.clone());
                let status = job.map(|job| job.status).unwrap_or(CompactJobStatus::Idle);
                let was_busy = match ui.get_job_phase() {
                    CompactJobPhase::WaitingForModel
                    | CompactJobPhase::Queued
                    | CompactJobPhase::Compressing => true,
                    CompactJobPhase::Idle
                    | CompactJobPhase::Failed
                    | CompactJobPhase::SourceChanged
                    | CompactJobPhase::InterruptedByRecording => false,
                };
                let terminal = match status {
                    CompactJobStatus::Idle
                    | CompactJobStatus::Failed { .. }
                    | CompactJobStatus::SourceChanged
                    | CompactJobStatus::InterruptedByRecording => true,
                    CompactJobStatus::WaitingForModel
                    | CompactJobStatus::Queued
                    | CompactJobStatus::Compressing { .. } => false,
                };
                ui.set_job_phase(job_phase(&status));
                if was_busy && terminal {
                    let id = ui.get_meeting_id().to_string();
                    let ui_generation = ui.get_ui_generation().to_string();
                    let state = poll_state.clone();
                    let weak = weak.clone();
                    souffle_lib::async_runtime::spawn(async move {
                        let meeting = state.db.load_meeting(&id);
                        let service = state.compression.clone();
                        let _ = weak.upgrade_in_event_loop(move |w| {
                            if w.get_active_meeting_id().as_str() == id
                                && w.global::<CompressionUi>().get_ui_generation().as_str()
                                    == ui_generation
                                && service.job(&id).map(|job| job.generation) == generation
                            {
                                if let Ok(meeting) = meeting {
                                    populate(&w, &meeting);
                                }
                            }
                        });
                    });
                }
                match status {
                    CompactJobStatus::Compressing { completed, total } => {
                        ui.set_progress(if total == 0 {
                            0
                        } else {
                            (completed * 100 / total) as i32
                        })
                    }
                    CompactJobStatus::Failed { detail } => ui.set_job_error(detail.into()),
                    CompactJobStatus::Idle
                    | CompactJobStatus::WaitingForModel
                    | CompactJobStatus::Queued
                    | CompactJobStatus::SourceChanged
                    | CompactJobStatus::InterruptedByRecording => {}
                }
            }
        },
    );
    let download_state = state.clone();
    window.global::<CompressionUi>().on_download(move || {
        if souffle_lib::commands::settings_capture_busy(&download_state).unwrap_or(true) {
            return;
        }
        let service = download_state.compression.clone();
        let state = Arc::downgrade(&download_state);
        souffle_lib::async_runtime::spawn(async move {
            let _ = service.ensure_model(state).await;
        });
    });
    let service = state.compression.clone();
    window
        .global::<CompressionUi>()
        .on_cancel_download(move || service.cancel_install());
    let service = state.compression.clone();
    let weak = window.as_weak();
    window.global::<CompressionUi>().on_cancel(move || {
        if let Some(w) = weak.upgrade() {
            service.cancel(w.get_active_meeting_id().as_str());
        }
    });
    let generate_state = state.clone();
    let weak = window.as_weak();
    window.global::<CompressionUi>().on_generate(move || {
        let Some(w) = weak.upgrade() else { return };
        let id = w.get_active_meeting_id().to_string();
        if id.is_empty() {
            return;
        }
        let token = uuid::Uuid::new_v4().to_string();
        let ui = w.global::<CompressionUi>();
        ui.set_ui_generation(token.clone().into());
        ui.set_copied(false);
        ui.set_job_phase(CompactJobPhase::Queued);
        ui.set_job_error("".into());
        let state = generate_state.clone();
        let weak = weak.clone();
        souffle_lib::async_runtime::spawn(async move {
            let previous_generation = state.compression.job(&id).map(|job| job.generation);
            let result = souffle_lib::commands::generate_compact_transcript(
                state.clone(),
                id.clone(),
                None,
                true,
            )
            .await;
            let cancelled = state.compression.job(&id).is_some_and(|job| {
                Some(&job.generation) != previous_generation.as_ref()
                    && job.cancel.load(std::sync::atomic::Ordering::Relaxed)
            });
            let meeting = state.db.load_meeting(&id);
            let _ = weak.upgrade_in_event_loop(move |w| {
                if w.get_active_meeting_id().as_str() != id
                    || w.global::<CompressionUi>().get_ui_generation().as_str() != token
                {
                    return;
                }
                if let Ok(meeting) = meeting {
                    populate(&w, &meeting);
                }
                if let Err(error) = result {
                    if !cancelled {
                        w.global::<CompressionUi>().set_job_error(error.into());
                    }
                }
            });
        });
    });
    let copy_state = state.clone();
    let weak = window.as_weak();
    window.global::<CompressionUi>().on_copy(move || {
        let Some(w) = weak.upgrade() else { return };
        let id = w.get_active_meeting_id().to_string();
        let state = copy_state.clone();
        let weak = weak.clone();
        souffle_lib::async_runtime::spawn(async move {
            let result = state
                .db
                .load_compact_transcript(&id)
                .and_then(|c| c.ok_or("No compact transcript".into()))
                .and_then(|c| souffle_lib::clipboard::copy_text(&c.text));
            let _ = weak.upgrade_in_event_loop(move |w| {
                if w.get_active_meeting_id().as_str() == id {
                    match result {
                        Ok(()) => w.global::<CompressionUi>().set_copied(true),
                        Err(error) => w.global::<CompressionUi>().set_job_error(error.into()),
                    }
                }
            });
        });
    });
    let expand_state = state.clone();
    let weak = window.as_weak();
    window.global::<CompressionUi>().on_expand(move || {
        let Some(w) = weak.upgrade() else { return };
        let ui = w.global::<CompressionUi>();
        ui.set_expanded(!ui.get_expanded());
        if let Ok(meeting) = expand_state
            .db
            .load_meeting(w.get_active_meeting_id().as_str())
        {
            populate(&w, &meeting);
        }
    });
    let weak = window.as_weak();
    window.global::<CompressionUi>().on_seek(move |seconds| {
        if let Some(w) = weak.upgrade() {
            if let Ok(meeting) = state.db.load_meeting(w.get_active_meeting_id().as_str()) {
                if meeting.duration_seconds > 0. {
                    w.invoke_meeting_detail_audio_seek_requested(
                        (seconds as f64 / meeting.duration_seconds).clamp(0., 1.) as f32,
                    );
                }
            }
        }
    });
    timer
}

thread_local! { static APP_TIMER: std::cell::RefCell<Option<slint::Timer>> = const { std::cell::RefCell::new(None) }; }
pub fn install_for_app(window: &MainWindow, state: Arc<AppState>) {
    APP_TIMER.with(|timer| *timer.borrow_mut() = Some(install(window, state)));
}
