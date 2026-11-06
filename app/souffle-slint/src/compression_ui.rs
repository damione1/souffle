//! Native compression projection shared by the app and synthetic QA fixture.
use crate::{CompactJobPhase, CompactLine, CompressionModelPhase, CompressionUi, MainWindow};
use slint::ComponentHandle;
use souffle_lib::{
    models::compression::ModelStatus,
    settings::{AppSettings, SettingsOptions},
    state::AppState,
    summary::compress::{CompactJobStatus, CompressionError, CompressionInput},
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
/// Associate a compact turn with its exact original recording session. Edited,
/// stale, cross-session or unverified source spans deliberately have no seek target.
fn preview(
    meeting: &MeetingTranscript,
    compact: &souffle_lib::transcript::CompactTranscript,
) -> Vec<CompactLine> {
    use souffle_schema::paragraphs::{
        PAUSE_THRESHOLD_SECONDS, SegmentLike, group_into_paragraph_segments,
    };
    struct Indexed<'a> {
        index: usize,
        segment: &'a souffle_lib::engine::TranscriptionSegment,
    }
    impl SegmentLike for Indexed<'_> {
        fn text(&self) -> &str {
            &self.segment.text
        }
        fn start_time(&self) -> f64 {
            self.segment.start_time
        }
        fn end_time(&self) -> f64 {
            self.segment.end_time
        }
        fn speaker(&self) -> Option<souffle_lib::engine::Speaker> {
            self.segment.speaker
        }
    }
    let source = CompressionInput::from_meeting(meeting);
    let trusted =
        !compact.is_stale && source.segmented && source.source_sha256 == compact.source_sha256;
    let indexed: Vec<_> = meeting
        .segments
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.text.trim().is_empty())
        .map(|(index, segment)| Indexed { index, segment })
        .collect();
    let groups = if trusted {
        group_into_paragraph_segments(&indexed, PAUSE_THRESHOLD_SECONDS)
    } else {
        Vec::new()
    };
    let count = compact.text.lines().count();
    compact
        .text
        .lines()
        .enumerate()
        .map(|(line_index, line)| {
            let seek = if trusted && groups.len() == count {
                groups.get(line_index).and_then(|group| {
                    let first = group.first()?;
                    let session_for =
                        |index| {
                            let mut matches = meeting.recording_sessions.iter().enumerate().filter(
                                |(_, session)| {
                                    index >= session.start_segment_index as usize
                                        && index < session.end_segment_index as usize
                                },
                            );
                            let (session, _) = matches.next()?;
                            if matches.next().is_some() {
                                None
                            } else {
                                Some(session)
                            }
                        };
                    let session = session_for(first.index)?;
                    if !group
                        .iter()
                        .all(|segment| session_for(segment.index) == Some(session))
                    {
                        return None;
                    }
                    let original = source.lines.get(line_index)?;
                    if line.split_once(']').map(|(prefix, _)| prefix)
                        != original.split_once(']').map(|(prefix, _)| prefix)
                    {
                        return None;
                    }
                    let seconds = first.segment.start_time;
                    if !seconds.is_finite() || seconds < 0. {
                        return None;
                    }
                    Some((i32::try_from(session).ok()?, seconds as f32))
                })
            } else {
                None
            };
            let (session_index, seconds) = seek.unwrap_or_default();
            CompactLine {
                text: line.into(),
                seconds,
                timed: seek.is_some(),
                session_index,
            }
        })
        .collect()
}
/// Publish compact metadata and a virtualized row model from a coherent meeting
/// snapshot. Full rows are cached by the model; expanding never rereads segments.
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
    ui.set_has_source(
        CompressionInput::from_meeting(meeting)
            .lines
            .iter()
            .any(|line| !line.trim().is_empty()),
    );
    ui.set_has_compact(meeting.compact_transcript.is_some());
    if let Some(compact) = &meeting.compact_transcript {
        ui.set_stale(compact.is_stale);
        ui.set_generated_at(compact.generated_at.clone().into());
        ui.set_compact_rate((compact.rate * 100.).round() as i32);
        ui.set_approx_tokens(compact.approx_tokens.min(i32::MAX as usize) as i32);
        ui.set_lines(Rc::new(slint::VecModel::from(preview(meeting, compact))).into());
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
                                && let Ok(meeting) = meeting
                            {
                                populate(&w, &meeting);
                            }
                        });
                    });
                }
                match status {
                    CompactJobStatus::Compressing { completed, total } => ui.set_progress(
                        completed
                            .saturating_mul(100)
                            .checked_div(total)
                            .unwrap_or(0)
                            .min(100) as i32,
                    ),
                    CompactJobStatus::Failed { error } => {
                        ui.set_job_error(error.to_string().into())
                    }
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
            let result = souffle_lib::commands::generate_compact_transcript(
                state.clone(),
                id.clone(),
                None,
                true,
            )
            .await;
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
                    match error {
                        CompressionError::Cancelled => {}
                        CompressionError::InterruptedByRecording
                        | CompressionError::CaptureBusy
                        | CompressionError::RuntimeBusy
                        | CompressionError::SourceChanged
                        | CompressionError::Deleted
                        | CompressionError::Ongoing
                        | CompressionError::EmptyTranscript
                        | CompressionError::InvalidRate
                        | CompressionError::ModelUnavailable
                        | CompressionError::AlreadyRunning
                        | CompressionError::ModelPreparation { .. }
                        | CompressionError::ModelValidation { .. }
                        | CompressionError::Inference { .. }
                        | CompressionError::Database { .. }
                        | CompressionError::Runtime { .. } => w
                            .global::<CompressionUi>()
                            .set_job_error(error.to_string().into()),
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
    let weak = window.as_weak();
    window.global::<CompressionUi>().on_expand(move || {
        if let Some(w) = weak.upgrade() {
            let ui = w.global::<CompressionUi>();
            ui.set_expanded(!ui.get_expanded());
        }
    });
    let weak = window.as_weak();
    window
        .global::<CompressionUi>()
        .on_seek(move |session_index, seconds| {
            if let Some(w) = weak.upgrade()
                && w.get_active_meeting_id() == w.global::<CompressionUi>().get_meeting_id()
                && session_index >= 0
                && seconds.is_finite()
                && seconds >= 0.
            {
                w.invoke_meeting_detail_transcript_paragraph_clicked(session_index, seconds);
            }
        });
    timer
}

thread_local! { static APP_TIMER: std::cell::RefCell<Option<slint::Timer>> = const { std::cell::RefCell::new(None) }; }
pub fn install_for_app(window: &MainWindow, state: Arc<AppState>) {
    APP_TIMER.with(|timer| *timer.borrow_mut() = Some(install(window, state)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use souffle_lib::{
        engine::{Speaker, TranscriptionSegment},
        transcript::{CompactTranscript, MeetingRecordingSession},
    };
    fn fixture() -> MeetingTranscript {
        let now = chrono::Utc::now();
        let segment = |speaker, text: &str| TranscriptionSegment {
            text: text.into(),
            start_time: 12.4,
            end_time: 13.,
            is_final: true,
            language: None,
            confidence: None,
            speaker: Some(speaker),
        };
        MeetingTranscript {
            id: "seek-fixture".into(),
            title: "Synthetic".into(),
            started_at: now,
            ended_at: Some(now),
            duration_seconds: 200.,
            transcription_profile: Default::default(),
            recording_sessions: vec![
                MeetingRecordingSession::completed("first".into(), now, now, 0, 1),
                MeetingRecordingSession::completed("resumed".into(), now, now, 1, 2),
            ],
            segments: vec![
                segment(Speaker::Me, "First session."),
                segment(Speaker::Them, "Resumed session."),
            ],
            summary: None,
            summary_is_stale: false,
            summary_model: None,
            summary_generated_at: None,
            structured_summary: None,
            compact_transcript: None,
            edited_transcript: None,
            notes: None,
            calendar_event_id: None,
            participants: vec![],
            system_audio: None,
        }
    }
    fn compact(meeting: &MeetingTranscript) -> CompactTranscript {
        let source = CompressionInput::from_meeting(meeting);
        CompactTranscript {
            text: source.lines.join("\n"),
            model: "synthetic".into(),
            rate: 0.5,
            generated_at: chrono::Utc::now().to_rfc3339(),
            is_stale: false,
            source_sha256: source.source_sha256,
            approx_tokens: 10,
        }
    }
    #[test]
    fn compact_preview_seek_keeps_session_identity_and_disables_ambiguous_sources() {
        let mut meeting = fixture();
        let mut saved = compact(&meeting);
        let rows = preview(&meeting, &saved);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.timed));
        assert_eq!((rows[0].session_index, rows[1].session_index), (0, 1));
        assert_eq!((rows[0].seconds, rows[1].seconds), (12.4, 12.4));
        saved.is_stale = true;
        assert!(preview(&meeting, &saved).iter().all(|row| !row.timed));
        meeting.edited_transcript = Some("[0:12] Me: edited text with a timestamp".into());
        saved = compact(&meeting);
        assert!(preview(&meeting, &saved).iter().all(|row| !row.timed));
        meeting.edited_transcript = None;
        meeting.segments[1].speaker = Some(Speaker::Me);
        meeting.segments[1].start_time = 13.1;
        meeting.segments[1].end_time = 13.5;
        saved = compact(&meeting);
        let cross_session = preview(&meeting, &saved);
        assert_eq!(cross_session.len(), 1);
        assert!(!cross_session[0].timed);
    }
}
