use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};

use crossbeam_channel::Sender;
use tauri::AppHandle;
use tauri_specta::Event;
use tracing::{debug, error, info, warn};

use crate::app_events::StateChanged;
use crate::audio::system_activity::SystemAudioActivity;
use crate::db::Database;
use crate::engine::{TranscriptionProfile, TranscriptionSegment};
use crate::lock_ext::MutexExt;
use crate::pipeline::EngineActorHandle;
use crate::state_machine::{AppStateMachine, StateAction};
use crate::transcript::{MeetingParticipant, MeetingRecordingSession, MeetingTranscript};

/// Commands sent to the audio thread
pub enum AudioCommand {
    Start {
        session_id: u64,
        target_sample_rate: u32,
        mic_gain: f32,
        /// Meeting mode: also capture system audio via a Core Audio tap
        /// and mix it with the microphone.
        capture_system_audio: bool,
        /// Diarized meeting: emit the mic (Me) and system audio (Them) as two
        /// source-tagged streams instead of one mixed stream, so each can be
        /// transcribed by its own engine. Only meaningful with system audio.
        diarize: bool,
        /// Where to record this meeting's mixed audio, if the retention
        /// setting is not off. `None` for dictation and for meetings
        /// recorded with retention off.
        record_path: Option<std::path::PathBuf>,
    },
    Stop,
    SelectDevice(String),
    /// Configure (or clear) the preferred microphone to use while the lid is
    /// closed with an external display attached (clamshell mode). Sent at
    /// startup and whenever the setting changes; `None` means "just follow
    /// the system default", the previous behavior.
    SetClamshellDevice(Option<String>),
    /// Refresh input priority policy and anti-Bluetooth preference. Sent at
    /// startup and whenever related settings change.
    SetInputPolicy {
        priority: crate::audio::InputPriority,
        allow_bluetooth_mic: bool,
    },
    /// Give the audio thread an AppHandle so meeting mode can emit
    /// SystemAudioStatus events.
    AttachApp(AppHandle),
    /// Re-run input resolution and hot-swap the mic leg when needed (device
    /// list or default-input change, priority update, explicit pin change).
    RefreshInputRoute,
}

/// Accumulated meeting segments while recording
pub struct MeetingAccumulator {
    pub id: String,
    pub title: String,
    pub existing_segments: Vec<TranscriptionSegment>,
    pub new_segments: Vec<TranscriptionSegment>,
    pub recording_sessions: Vec<MeetingRecordingSession>,
    pub session_started_at: chrono::DateTime<chrono::Utc>,
    pub transcription_profile: TranscriptionProfile,
    pub summary: Option<String>,
    pub summary_is_stale: bool,
    pub summary_model: Option<String>,
    pub summary_generated_at: Option<chrono::DateTime<chrono::Utc>>,
    pub structured_summary: Option<crate::transcript::StructuredSummary>,
    /// Notes the user types while the meeting records; persisted at stop.
    pub notes: Option<String>,
    /// Calendar event this meeting was started from, when any.
    pub calendar_event_id: Option<String>,
    /// Attendees captured from the calendar event.
    pub participants: Vec<MeetingParticipant>,
    /// How many of `new_segments` have already been flushed to the DB by the
    /// incremental persistence path. Lets a crash mid-meeting lose at most the
    /// last unflushed batch instead of the whole session.
    pub persisted_new_count: usize,
}

impl MeetingAccumulator {
    /// Build the persisted transcript for this accumulator, appending one
    /// completed recording session ending at `ended_at`. Also used by the
    /// engine actor (via `AppState::abort_active_session`) to salvage a
    /// meeting when its recording session aborts mid-way.
    pub fn into_transcript(self, ended_at: chrono::DateTime<chrono::Utc>) -> MeetingTranscript {
        let mut segments = self.existing_segments;
        let start_segment_index = segments.len() as u64;
        let had_new_segments = !self.new_segments.is_empty();
        let has_summary = self.summary.is_some();
        segments.extend(self.new_segments);
        let end_segment_index = segments.len() as u64;

        let mut recording_sessions = self.recording_sessions;
        recording_sessions.push(MeetingRecordingSession::completed(
            uuid::Uuid::new_v4().to_string(),
            self.session_started_at,
            ended_at,
            start_segment_index,
            end_segment_index,
        ));

        let started_at = recording_sessions
            .first()
            .map(|session| session.started_at)
            .unwrap_or(self.session_started_at);
        let ended_at = recording_sessions.last().map(|session| session.ended_at);
        let duration_seconds = recording_sessions
            .iter()
            .map(|session| session.duration_seconds)
            .sum();

        MeetingTranscript {
            id: self.id,
            title: self.title,
            started_at,
            ended_at,
            duration_seconds,
            transcription_profile: self.transcription_profile,
            recording_sessions,
            segments,
            summary: self.summary,
            summary_is_stale: self.summary_is_stale || (has_summary && had_new_segments),
            summary_model: self.summary_model,
            summary_generated_at: self.summary_generated_at,
            structured_summary: if had_new_segments {
                None
            } else {
                self.structured_summary
            },
            edited_transcript: None,
            notes: self.notes,
            calendar_event_id: self.calendar_event_id,
            participants: self.participants,
        }
    }
}

/// Shared application state, managed by Tauri.
/// AudioCapture lives on its own thread (cpal Stream is !Send on macOS),
/// and the transcription engine lives on the engine actor thread —
/// both are driven via command channels.
pub struct AppState {
    pub audio_cmd_sender: Sender<AudioCommand>,
    /// Wrapped in Arc so async commands can clone a handle into a blocking
    /// task (`spawn_blocking`) and keep the long crossbeam reply waits off the
    /// Tauri command thread — otherwise they freeze the window event loop.
    pub engine_actor: Arc<EngineActorHandle>,
    pub next_audio_session_id: Mutex<u64>,
    pub meeting_accumulator: Arc<Mutex<Option<MeetingAccumulator>>>,
    pub db: Arc<Database>,
    /// Latest audio RMS level (0.0-1.0), stored as f32 bits in AtomicU32
    pub audio_rms: Arc<AtomicU32>,
    /// Passive system-audio activity timestamps for calendar auto-start nudges.
    pub system_audio_activity: Arc<SystemAudioActivity>,
    /// Unified state machine — the source of truth for app lifecycle
    pub machine: Mutex<AppStateMachine>,
    /// Tauri app handle for emitting events (set during setup)
    pub app_handle: Mutex<Option<AppHandle>>,
    /// Set when a meeting recording is stopped by the system-sleep handler
    /// (as opposed to a user-initiated stop), so the frontend can offer to
    /// resume it after wake. Cleared by `take_sleep_paused_meeting`.
    pub sleep_paused_meeting_id: Mutex<Option<String>>,
}

impl AppState {
    pub fn new(
        audio_cmd_sender: Sender<AudioCommand>,
        engine_actor: Arc<EngineActorHandle>,
        db: Arc<Database>,
        audio_rms: Arc<AtomicU32>,
        system_audio_activity: Arc<SystemAudioActivity>,
    ) -> Self {
        Self {
            audio_cmd_sender,
            engine_actor,
            next_audio_session_id: Mutex::new(0),
            meeting_accumulator: Arc::new(Mutex::new(None)),
            db,
            audio_rms,
            system_audio_activity,
            machine: Mutex::new(AppStateMachine::Idle),
            app_handle: Mutex::new(None),
            sleep_paused_meeting_id: Mutex::new(None),
        }
    }

    /// Apply a state transition, update the machine, and emit a StateChanged event.
    ///
    /// INVARIANT: never call window/AppKit operations (they dispatch to the
    /// main thread and block until it services them) while holding an
    /// AppState mutex. A `#[tauri::command]` running on the main thread may
    /// be waiting on that very mutex (e.g. `state.app_handle()`), which
    /// deadlocks both threads permanently: this method waits forever for the
    /// main thread to drain its queue, and the main thread waits forever for
    /// this method to release the lock. So the `machine` lock is scoped to
    /// just the transition itself, and the `AppHandle` is cloned out of its
    /// lock before any emit/sync call runs.
    pub fn apply_transition(&self, action: StateAction) -> Result<AppStateMachine, String> {
        let new_state = {
            let mut machine = self.machine.acquire()?;
            let new_state = machine.clone().transition(action)?;
            debug!(
                from = machine.variant_name(),
                to = new_state.variant_name(),
                "State transition"
            );
            *machine = new_state.clone();
            new_state
        };

        let handle = self.app_handle.acquire()?.as_ref().cloned();
        if let Some(handle) = handle {
            let _ = StateChanged(new_state.clone()).emit(&handle);
            crate::pill::sync(&handle, &new_state);
            crate::tray::sync(&handle, &new_state);
        }

        Ok(new_state)
    }

    /// Get a clone of the current machine state.
    pub fn current_machine_state(&self) -> Result<AppStateMachine, String> {
        Ok(self.machine.acquire()?.clone())
    }

    /// Clone the stored AppHandle (set during setup). Used by async commands to
    /// drive background finalization tasks that re-fetch `AppState` off-thread.
    pub fn app_handle(&self) -> Result<AppHandle, String> {
        self.app_handle
            .acquire()?
            .as_ref()
            .cloned()
            .ok_or_else(|| "App handle not set".to_string())
    }

    /// The engine actor unloaded an idle model to reclaim memory. Mirrors the
    /// Unload/UnloadComplete pair already used when swapping models, so the
    /// state machine (and therefore the frontend) never disagrees with the
    /// actor about whether a model is actually loaded. A no-op (logged) if
    /// the machine isn't in `Ready` (e.g. a recording start raced the timer).
    pub fn unload_idle_model(&self) {
        if let Err(e) = self.apply_transition(StateAction::Unload { next_profile: None }) {
            warn!("Idle model unload: failed to start transition: {e}");
            return;
        }
        if let Err(e) = self.apply_transition(StateAction::UnloadComplete) {
            warn!("Idle model unload: failed to complete transition: {e}");
        }
    }

    /// A session died mid-recording: stop audio capture, salvage any
    /// in-progress meeting to the DB, and fail the state machine so the UI
    /// leaves the recording state. Called by the engine actor after it emits
    /// the PipelineError event (the pipeline layer owns that event; this is
    /// the app-level cleanup that follows it).
    pub fn abort_active_session(&self, message: String) {
        let _ = self.audio_cmd_sender.send(AudioCommand::Stop);

        // Salvage an in-progress meeting: stop_meeting_recording can no
        // longer run once the machine is in Error, so the accumulated
        // segments would otherwise be lost.
        let accumulator = self
            .meeting_accumulator
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());
        if let Some(meeting) = accumulator {
            let transcript = meeting.into_transcript(chrono::Utc::now());
            match self.db.save_meeting(&transcript) {
                Ok(()) => info!(
                    id = %transcript.id,
                    "Meeting salvaged to history after session abort"
                ),
                Err(e) => error!("Failed to salvage meeting after session abort: {e}"),
            }
        }

        if let Err(e) = self.apply_transition(StateAction::Fail { message }) {
            warn!("Failed to apply Fail transition after session abort: {e}");
        }
    }

    /// Remember that a meeting recording was stopped by the system-sleep
    /// handler, so the frontend can offer to resume it once the system wakes.
    pub fn set_sleep_paused_meeting(&self, meeting_id: String) {
        if let Ok(mut guard) = self.sleep_paused_meeting_id.lock() {
            *guard = Some(meeting_id);
        }
    }

    /// Return and clear the meeting id paused by sleep, if any. Clearing on
    /// read means a resume attempt (successful or not) never re-offers the
    /// same meeting on a later wake.
    pub fn take_sleep_paused_meeting(&self) -> Option<String> {
        self.sleep_paused_meeting_id
            .lock()
            .ok()
            .and_then(|mut guard| guard.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::default_transcription_profile;
    use crate::transcript::MeetingRecordingSession;
    use chrono::Duration;

    #[test]
    fn into_transcript_appends_resumed_session_and_marks_summary_stale() {
        let first_start = chrono::Utc::now();
        let first_end = first_start + Duration::seconds(30);
        let second_start = first_end + Duration::minutes(5);
        let second_end = second_start + Duration::seconds(45);

        let accumulator = MeetingAccumulator {
            id: "meeting-1".to_string(),
            title: "Roadmap".to_string(),
            existing_segments: vec![TranscriptionSegment {
                text: "Existing".to_string(),
                start_time: 0.0,
                end_time: 1.0,
                is_final: true,
                language: None,
                confidence: None,
                speaker: None,
            }],
            new_segments: vec![TranscriptionSegment {
                text: "Appended".to_string(),
                start_time: 1.0,
                end_time: 2.0,
                is_final: true,
                language: None,
                confidence: None,
                speaker: None,
            }],
            recording_sessions: vec![MeetingRecordingSession::completed(
                "session-1".to_string(),
                first_start,
                first_end,
                0,
                1,
            )],
            session_started_at: second_start,
            transcription_profile: default_transcription_profile(),
            summary: Some("Old summary".to_string()),
            summary_is_stale: false,
            summary_model: Some("qwen".to_string()),
            summary_generated_at: Some(first_end),
            structured_summary: Some(crate::transcript::StructuredSummary {
                decisions: vec!["Old decision".to_string()],
                action_items: vec![],
                open_questions: vec![],
            }),
            notes: None,
            calendar_event_id: None,
            participants: Vec::new(),
            persisted_new_count: 0,
        };

        let transcript = accumulator.into_transcript(second_end);

        assert_eq!(transcript.id, "meeting-1");
        assert_eq!(transcript.recording_sessions.len(), 2);
        assert_eq!(transcript.recording_sessions[1].start_segment_index, 1);
        assert_eq!(transcript.recording_sessions[1].end_segment_index, 2);
        assert_eq!(transcript.segments.len(), 2);
        assert!(transcript.summary_is_stale);
        assert!(transcript.structured_summary.is_none());
        assert_eq!(transcript.started_at, first_start);
        assert_eq!(transcript.ended_at, Some(second_end));
        assert_eq!(transcript.duration_seconds, 75.0);
    }

    #[test]
    fn into_transcript_preserves_fresh_summary_when_no_new_segments_arrive() {
        let started_at = chrono::Utc::now();
        let ended_at = started_at + Duration::seconds(10);

        let accumulator = MeetingAccumulator {
            id: "meeting-2".to_string(),
            title: "Silent Resume".to_string(),
            existing_segments: Vec::new(),
            new_segments: Vec::new(),
            recording_sessions: Vec::new(),
            session_started_at: started_at,
            transcription_profile: default_transcription_profile(),
            summary: Some("Still current".to_string()),
            summary_is_stale: false,
            summary_model: Some("qwen".to_string()),
            summary_generated_at: Some(started_at),
            structured_summary: None,
            notes: None,
            calendar_event_id: None,
            participants: Vec::new(),
            persisted_new_count: 0,
        };

        let transcript = accumulator.into_transcript(ended_at);

        assert_eq!(transcript.recording_sessions.len(), 1);
        assert_eq!(transcript.duration_seconds, 10.0);
        assert!(!transcript.summary_is_stale);
    }
}
