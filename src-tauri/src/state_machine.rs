use serde::{Deserialize, Serialize};

use crate::engine::TranscriptionProfile;

/// Unified application state machine.
/// Replaces scattered `is_recording`, `model_loaded`, `recording_mode`, `active_profile` booleans
/// with a single enum that enforces valid transitions.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(tag = "state", content = "data")]
#[serde(rename_all = "snake_case")]
pub enum AppStateMachine {
    Idle,
    Downloading {
        profile: TranscriptionProfile,
    },
    Downloaded {
        profile: TranscriptionProfile,
    },
    Loading {
        profile: TranscriptionProfile,
    },
    Ready {
        profile: TranscriptionProfile,
    },
    RecordingDictation {
        profile: TranscriptionProfile,
        session_id: u64,
    },
    RecordingMeeting {
        profile: TranscriptionProfile,
        session_id: u64,
        meeting_id: String,
    },
    Stopping {
        profile: TranscriptionProfile,
        was_recording: RecordingKind,
    },
    Unloading {
        profile: TranscriptionProfile,
        next_profile: Option<TranscriptionProfile>,
    },
    Error {
        message: String,
        recovery: ErrorRecovery,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum RecordingKind {
    Dictation,
    Meeting { meeting_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ErrorRecovery {
    RetryFromIdle,
    RetryFromDownloaded { profile: TranscriptionProfile },
    RetryFromReady { profile: TranscriptionProfile },
}

/// Internal-only actions that drive state transitions.
pub enum StateAction {
    StartDownload { profile: TranscriptionProfile },
    DownloadComplete,
    StartLoad,
    LoadComplete,
    StartDictation { session_id: u64 },
    StartMeeting { session_id: u64, meeting_id: String },
    StopRecording,
    StopComplete,
    Unload { next_profile: Option<TranscriptionProfile> },
    UnloadComplete,
    Fail { message: String },
    Recover,
}

impl AppStateMachine {
    /// Apply an action to the current state, returning the new state or an error
    /// if the transition is invalid.
    pub fn transition(self, action: StateAction) -> Result<AppStateMachine, String> {
        use AppStateMachine::*;
        use StateAction::*;

        match (self, action) {
            // --- Download transitions ---
            (Idle, StartDownload { profile }) => Ok(Downloading {
                profile,
            }),
            (Downloaded { profile: current }, StartDownload { profile }) if current != profile => {
                Ok(Downloading { profile })
            }
            (Downloading { profile }, DownloadComplete) => Ok(Downloaded { profile }),
            (Downloading { .. }, Fail { message }) => Ok(Error {
                message,
                recovery: ErrorRecovery::RetryFromIdle,
            }),

            // --- Load transitions ---
            (Downloaded { profile }, StartLoad) => Ok(Loading {
                profile: profile.clone(),
            }),
            (Loading { profile }, LoadComplete) => Ok(Ready { profile }),
            (Loading { profile }, Fail { message }) => Ok(Error {
                message,
                recovery: ErrorRecovery::RetryFromDownloaded { profile },
            }),

            // --- Recording transitions ---
            (Ready { profile }, StartDictation { session_id }) => Ok(RecordingDictation {
                profile,
                session_id,
            }),
            (Ready { profile }, StartMeeting { session_id, meeting_id }) => {
                Ok(RecordingMeeting {
                    profile,
                    session_id,
                    meeting_id,
                })
            }

            // --- Stop recording ---
            (RecordingDictation { profile, .. }, StopRecording) => Ok(Stopping {
                profile,
                was_recording: RecordingKind::Dictation,
            }),
            (RecordingMeeting { profile, meeting_id, .. }, StopRecording) => Ok(Stopping {
                profile,
                was_recording: RecordingKind::Meeting { meeting_id },
            }),
            (Stopping { profile, .. }, StopComplete) => Ok(Ready { profile }),
            // Idempotent: the decoupled stop finalizes in a background task, so a
            // late/duplicate StopComplete (e.g. after an abort already moved the
            // machine on) must not wedge or error noisily.
            (Ready { profile }, StopComplete) => Ok(Ready { profile }),
            // `Fail` is total over every model-loaded state. The async/decoupled
            // stop widens the `Stopping` window, so an abort can land while the
            // machine is Stopping (or already Ready) — without these arms it would
            // hit the invalid-transition catch-all and stick in `Stopping`.
            (RecordingDictation { profile, .. }, Fail { message })
            | (RecordingMeeting { profile, .. }, Fail { message })
            | (Stopping { profile, .. }, Fail { message })
            | (Ready { profile }, Fail { message }) => Ok(Error {
                message,
                recovery: ErrorRecovery::RetryFromReady { profile },
            }),

            // --- Unload / model swap ---
            (Ready { profile }, Unload { next_profile }) => Ok(Unloading {
                profile,
                next_profile,
            }),
            (Unloading { next_profile: Some(next), .. }, UnloadComplete) => Ok(Loading {
                profile: next,
            }),
            (Unloading { profile, next_profile: None, .. }, UnloadComplete) => {
                Ok(Downloaded { profile })
            }

            // --- Error recovery ---
            (Error { recovery: ErrorRecovery::RetryFromIdle, .. }, Recover) => Ok(Idle),
            (Error { recovery: ErrorRecovery::RetryFromDownloaded { profile }, .. }, Recover) => {
                Ok(Downloaded { profile })
            }
            (Error { recovery: ErrorRecovery::RetryFromReady { profile }, .. }, Recover) => {
                Ok(Ready { profile })
            }

            // --- Already in target state (idempotent) ---
            (Downloaded { profile: ref current }, StartDownload { ref profile }) if current == profile => {
                Ok(Downloaded { profile: profile.clone() })
            }

            // --- Invalid transition ---
            (state, _action) => Err(format!(
                "Invalid state transition: cannot apply action from '{}'",
                state.variant_name(),
            )),
        }
    }

    pub fn is_recording(&self) -> bool {
        matches!(
            self,
            AppStateMachine::RecordingDictation { .. }
                | AppStateMachine::RecordingMeeting { .. }
                | AppStateMachine::Stopping { .. }
        )
    }

    pub fn is_model_ready(&self) -> bool {
        matches!(
            self,
            AppStateMachine::Ready { .. }
                | AppStateMachine::RecordingDictation { .. }
                | AppStateMachine::RecordingMeeting { .. }
                | AppStateMachine::Stopping { .. }
        )
    }

    pub fn active_profile(&self) -> Option<&TranscriptionProfile> {
        match self {
            AppStateMachine::Downloading { profile }
            | AppStateMachine::Downloaded { profile }
            | AppStateMachine::Loading { profile }
            | AppStateMachine::Ready { profile }
            | AppStateMachine::RecordingDictation { profile, .. }
            | AppStateMachine::RecordingMeeting { profile, .. }
            | AppStateMachine::Stopping { profile, .. }
            | AppStateMachine::Unloading { profile, .. } => Some(profile),
            AppStateMachine::Idle | AppStateMachine::Error { .. } => None,
        }
    }

    pub fn variant_name(&self) -> &'static str {
        match self {
            AppStateMachine::Idle => "idle",
            AppStateMachine::Downloading { .. } => "downloading",
            AppStateMachine::Downloaded { .. } => "downloaded",
            AppStateMachine::Loading { .. } => "loading",
            AppStateMachine::Ready { .. } => "ready",
            AppStateMachine::RecordingDictation { .. } => "recording_dictation",
            AppStateMachine::RecordingMeeting { .. } => "recording_meeting",
            AppStateMachine::Stopping { .. } => "stopping",
            AppStateMachine::Unloading { .. } => "unloading",
            AppStateMachine::Error { .. } => "error",
        }
    }

    /// Derive the legacy runtime phase from the current FSM state.
    pub fn runtime_phase(&self) -> crate::engine::TranscriptionRuntimePhase {
        use crate::engine::TranscriptionRuntimePhase;
        match self {
            AppStateMachine::Idle | AppStateMachine::Downloading { .. } => {
                TranscriptionRuntimePhase::DownloadRequired
            }
            AppStateMachine::Downloaded { .. } | AppStateMachine::Loading { .. } => {
                TranscriptionRuntimePhase::LoadRequired
            }
            AppStateMachine::Ready { .. }
            | AppStateMachine::RecordingDictation { .. }
            | AppStateMachine::RecordingMeeting { .. }
            | AppStateMachine::Stopping { .. }
            | AppStateMachine::Unloading { .. } => TranscriptionRuntimePhase::Ready,
            AppStateMachine::Error { recovery, .. } => match recovery {
                ErrorRecovery::RetryFromIdle => TranscriptionRuntimePhase::DownloadRequired,
                ErrorRecovery::RetryFromDownloaded { .. } => {
                    TranscriptionRuntimePhase::LoadRequired
                }
                ErrorRecovery::RetryFromReady { .. } => TranscriptionRuntimePhase::Ready,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::default_transcription_profile;

    fn test_profile() -> TranscriptionProfile {
        default_transcription_profile()
    }

    fn other_profile() -> TranscriptionProfile {
        TranscriptionProfile {
            model_id: "stt-2.6b-en".to_string(),
            model_label: "STT 2.6B EN".to_string(),
            ..default_transcription_profile()
        }
    }

    // --- Happy path: full lifecycle ---

    #[test]
    fn idle_to_downloading_to_downloaded() {
        let state = AppStateMachine::Idle;
        let state = state
            .transition(StateAction::StartDownload {
                profile: test_profile(),
            })
            .unwrap();
        assert!(matches!(state, AppStateMachine::Downloading { .. }));
        let state = state.transition(StateAction::DownloadComplete).unwrap();
        assert!(matches!(state, AppStateMachine::Downloaded { .. }));
    }

    #[test]
    fn downloaded_to_loading_to_ready() {
        let state = AppStateMachine::Downloaded {
            profile: test_profile(),
        };
        let state = state.transition(StateAction::StartLoad).unwrap();
        assert!(matches!(state, AppStateMachine::Loading { .. }));
        let state = state.transition(StateAction::LoadComplete).unwrap();
        assert!(matches!(state, AppStateMachine::Ready { .. }));
    }

    #[test]
    fn ready_to_recording_dictation_to_stopping_to_ready() {
        let state = AppStateMachine::Ready {
            profile: test_profile(),
        };
        let state = state
            .transition(StateAction::StartDictation { session_id: 1 })
            .unwrap();
        assert!(matches!(state, AppStateMachine::RecordingDictation { .. }));
        assert!(state.is_recording());
        let state = state.transition(StateAction::StopRecording).unwrap();
        assert!(matches!(state, AppStateMachine::Stopping { .. }));
        assert!(state.is_recording());
        let state = state.transition(StateAction::StopComplete).unwrap();
        assert!(matches!(state, AppStateMachine::Ready { .. }));
        assert!(!state.is_recording());
    }

    #[test]
    fn ready_to_recording_meeting_to_stopping_to_ready() {
        let state = AppStateMachine::Ready {
            profile: test_profile(),
        };
        let state = state
            .transition(StateAction::StartMeeting {
                session_id: 1,
                meeting_id: "m1".to_string(),
            })
            .unwrap();
        assert!(matches!(state, AppStateMachine::RecordingMeeting { .. }));
        let state = state.transition(StateAction::StopRecording).unwrap();
        assert!(matches!(
            state,
            AppStateMachine::Stopping {
                was_recording: RecordingKind::Meeting { .. },
                ..
            }
        ));
        let state = state.transition(StateAction::StopComplete).unwrap();
        assert!(matches!(state, AppStateMachine::Ready { .. }));
    }

    // --- Model swap ---

    #[test]
    fn ready_unload_with_next_profile_loads_new() {
        let state = AppStateMachine::Ready {
            profile: test_profile(),
        };
        let next = other_profile();
        let state = state
            .transition(StateAction::Unload {
                next_profile: Some(next.clone()),
            })
            .unwrap();
        assert!(matches!(state, AppStateMachine::Unloading { .. }));
        let state = state.transition(StateAction::UnloadComplete).unwrap();
        match &state {
            AppStateMachine::Loading { profile } => assert_eq!(profile.model_id, next.model_id),
            other => panic!("Expected Loading, got {:?}", other),
        }
    }

    #[test]
    fn ready_unload_without_next_returns_to_downloaded() {
        let state = AppStateMachine::Ready {
            profile: test_profile(),
        };
        let state = state
            .transition(StateAction::Unload {
                next_profile: None,
            })
            .unwrap();
        let state = state.transition(StateAction::UnloadComplete).unwrap();
        assert!(matches!(state, AppStateMachine::Downloaded { .. }));
    }

    // --- Error and recovery ---

    #[test]
    fn download_failure_and_recovery() {
        let state = AppStateMachine::Downloading {
            profile: test_profile(),
        };
        let state = state
            .transition(StateAction::Fail {
                message: "network".into(),
            })
            .unwrap();
        assert!(matches!(state, AppStateMachine::Error { .. }));
        let state = state.transition(StateAction::Recover).unwrap();
        assert!(matches!(state, AppStateMachine::Idle));
    }

    #[test]
    fn load_failure_and_recovery() {
        let profile = test_profile();
        let state = AppStateMachine::Loading {
            profile: profile.clone(),
        };
        let state = state
            .transition(StateAction::Fail {
                message: "oom".into(),
            })
            .unwrap();
        let state = state.transition(StateAction::Recover).unwrap();
        match state {
            AppStateMachine::Downloaded { profile: p } => {
                assert_eq!(p.model_id, profile.model_id)
            }
            other => panic!("Expected Downloaded, got {:?}", other),
        }
    }

    #[test]
    fn stopping_can_fail_without_wedging() {
        // The decoupled stop sits in Stopping during background finalize; an
        // abort that lands here must move to a recoverable Error, not stick.
        let state = AppStateMachine::Stopping {
            profile: test_profile(),
            was_recording: RecordingKind::Meeting {
                meeting_id: "m1".into(),
            },
        };
        let state = state
            .transition(StateAction::Fail {
                message: "audio gone".into(),
            })
            .unwrap();
        assert!(matches!(state, AppStateMachine::Error { .. }));
        let state = state.transition(StateAction::Recover).unwrap();
        assert!(matches!(state, AppStateMachine::Ready { .. }));
    }

    #[test]
    fn ready_can_fail_and_tolerates_late_stop_complete() {
        let profile = test_profile();
        // A late abort after stop already finalized (machine back to Ready).
        let failed = AppStateMachine::Ready {
            profile: profile.clone(),
        }
        .transition(StateAction::Fail {
            message: "late abort".into(),
        })
        .unwrap();
        assert!(matches!(failed, AppStateMachine::Error { .. }));

        // A duplicate/late StopComplete on Ready is a harmless no-op.
        let ready = AppStateMachine::Ready { profile }
            .transition(StateAction::StopComplete)
            .unwrap();
        assert!(matches!(ready, AppStateMachine::Ready { .. }));
    }

    #[test]
    fn recording_failure_and_recovery() {
        let profile = test_profile();
        let state = AppStateMachine::RecordingDictation {
            profile: profile.clone(),
            session_id: 1,
        };
        let state = state
            .transition(StateAction::Fail {
                message: "audio error".into(),
            })
            .unwrap();
        let state = state.transition(StateAction::Recover).unwrap();
        assert!(matches!(state, AppStateMachine::Ready { .. }));
    }

    // --- Invalid transitions ---

    #[test]
    fn idle_cannot_start_load() {
        let state = AppStateMachine::Idle;
        assert!(state.transition(StateAction::StartLoad).is_err());
    }

    #[test]
    fn idle_cannot_start_recording() {
        let state = AppStateMachine::Idle;
        assert!(state
            .transition(StateAction::StartDictation { session_id: 1 })
            .is_err());
    }

    #[test]
    fn recording_cannot_start_another_recording() {
        let state = AppStateMachine::RecordingDictation {
            profile: test_profile(),
            session_id: 1,
        };
        assert!(state
            .transition(StateAction::StartMeeting {
                session_id: 2,
                meeting_id: "m".into()
            })
            .is_err());
    }

    #[test]
    fn ready_cannot_stop_recording() {
        let state = AppStateMachine::Ready {
            profile: test_profile(),
        };
        assert!(state.transition(StateAction::StopRecording).is_err());
    }

    #[test]
    fn recording_cannot_unload() {
        let state = AppStateMachine::RecordingDictation {
            profile: test_profile(),
            session_id: 1,
        };
        assert!(state
            .transition(StateAction::Unload {
                next_profile: None
            })
            .is_err());
    }

    // --- Helper methods ---

    #[test]
    fn is_model_ready_for_ready_and_recording_states() {
        assert!(!AppStateMachine::Idle.is_model_ready());
        assert!(!AppStateMachine::Downloaded {
            profile: test_profile()
        }
        .is_model_ready());
        assert!(AppStateMachine::Ready {
            profile: test_profile()
        }
        .is_model_ready());
        assert!(AppStateMachine::RecordingDictation {
            profile: test_profile(),
            session_id: 1
        }
        .is_model_ready());
    }

    #[test]
    fn active_profile_returns_none_for_idle_and_error() {
        assert!(AppStateMachine::Idle.active_profile().is_none());
        assert!(AppStateMachine::Error {
            message: "x".into(),
            recovery: ErrorRecovery::RetryFromIdle
        }
        .active_profile()
        .is_none());
    }

    #[test]
    fn active_profile_returns_some_for_all_other_states() {
        let p = test_profile();
        assert!(AppStateMachine::Ready {
            profile: p.clone()
        }
        .active_profile()
        .is_some());
        assert!(AppStateMachine::RecordingDictation {
            profile: p,
            session_id: 1
        }
        .active_profile()
        .is_some());
    }

    #[test]
    fn runtime_phase_mapping() {
        use crate::engine::TranscriptionRuntimePhase;
        assert_eq!(
            AppStateMachine::Idle.runtime_phase(),
            TranscriptionRuntimePhase::DownloadRequired
        );
        assert_eq!(
            AppStateMachine::Downloaded {
                profile: test_profile()
            }
            .runtime_phase(),
            TranscriptionRuntimePhase::LoadRequired
        );
        assert_eq!(
            AppStateMachine::Ready {
                profile: test_profile()
            }
            .runtime_phase(),
            TranscriptionRuntimePhase::Ready
        );
    }

    #[test]
    fn idempotent_download_when_already_downloaded() {
        let profile = test_profile();
        let state = AppStateMachine::Downloaded {
            profile: profile.clone(),
        };
        let result = state.transition(StateAction::StartDownload {
            profile: profile.clone(),
        });
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), AppStateMachine::Downloaded { .. }));
    }

    #[test]
    fn download_different_profile_from_downloaded() {
        let state = AppStateMachine::Downloaded {
            profile: test_profile(),
        };
        let next = other_profile();
        let result = state.transition(StateAction::StartDownload {
            profile: next.clone(),
        });
        assert!(result.is_ok());
        match result.unwrap() {
            AppStateMachine::Downloading { profile } => {
                assert_eq!(profile.model_id, next.model_id)
            }
            other => panic!("Expected Downloading, got {:?}", other),
        }
    }

    #[test]
    fn error_recovery_then_download_and_load() {
        // Simulates the fix: recover from error, then re-download + load
        let profile = test_profile();
        let state = AppStateMachine::Error {
            message: "previous failure".into(),
            recovery: ErrorRecovery::RetryFromIdle,
        };
        let state = state.transition(StateAction::Recover).unwrap();
        assert!(matches!(state, AppStateMachine::Idle));

        let state = state
            .transition(StateAction::StartDownload {
                profile: profile.clone(),
            })
            .unwrap();
        let state = state.transition(StateAction::DownloadComplete).unwrap();
        let state = state.transition(StateAction::StartLoad).unwrap();
        let state = state.transition(StateAction::LoadComplete).unwrap();
        assert!(matches!(state, AppStateMachine::Ready { .. }));
    }

    #[test]
    fn error_cannot_start_load_directly() {
        let state = AppStateMachine::Error {
            message: "oops".into(),
            recovery: ErrorRecovery::RetryFromIdle,
        };
        assert!(state.transition(StateAction::StartLoad).is_err());
    }
}
