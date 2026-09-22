//! Download and install in-app updates via `native::updater` (SOU-191).
//!
//! Detection stays in [`crate::update_check`]: this module only executes.
//! Progress and ready bytes live here so a UI reload cannot lose them.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::app_events::UpdatePhase;
use crate::native::updater::ReadyUpdate;
use crate::state::AppState;
use crate::state_machine::AppStateMachine;

/// Why "Install and restart" must stay disabled / refuse before any side effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallBlockReason {
    RecordingDictation,
    RecordingMeeting,
    Stopping,
    Downloading,
    Loading,
    Unloading,
}

impl InstallBlockReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RecordingDictation => "recording_dictation",
            Self::RecordingMeeting => "recording_meeting",
            Self::Stopping => "stopping",
            Self::Downloading => "downloading",
            Self::Loading => "loading",
            Self::Unloading => "unloading",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateDownloadStatus {
    pub phase: UpdatePhase,
    pub version: Option<String>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
    /// Offer the GitHub release-page fallback when true.
    pub manual_fallback: bool,
}

struct DownloadState {
    phase: UpdatePhase,
    version: Option<String>,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    error: Option<String>,
    manual_fallback: bool,
    ready: Option<ReadyUpdate>,
}

impl Default for DownloadState {
    fn default() -> Self {
        Self {
            phase: UpdatePhase::Idle,
            version: None,
            downloaded_bytes: 0,
            total_bytes: None,
            error: None,
            manual_fallback: false,
            ready: None,
        }
    }
}

impl DownloadState {
    fn status(&self) -> UpdateDownloadStatus {
        UpdateDownloadStatus {
            phase: self.phase,
            version: self.version.clone(),
            downloaded_bytes: self.downloaded_bytes,
            total_bytes: self.total_bytes,
            error: self.error.clone(),
            manual_fallback: self.manual_fallback,
        }
    }
}

static DOWNLOAD: Mutex<DownloadState> = Mutex::new(DownloadState {
    phase: UpdatePhase::Idle,
    version: None,
    downloaded_bytes: 0,
    total_bytes: None,
    error: None,
    manual_fallback: false,
    ready: None,
});

static CANCEL_DOWNLOAD: AtomicBool = AtomicBool::new(false);
/// Bumped on each new download attempt and on cancel so a stale callback/result
/// cannot mutate the shared state of a later attempt.
static DOWNLOAD_ATTEMPT: AtomicU64 = AtomicU64::new(0);

/// Map a machine state to an install block, if any.
pub fn install_blocked_reason(machine: &AppStateMachine) -> Option<InstallBlockReason> {
    match machine {
        AppStateMachine::RecordingDictation { .. } => Some(InstallBlockReason::RecordingDictation),
        AppStateMachine::RecordingMeeting { .. } => Some(InstallBlockReason::RecordingMeeting),
        AppStateMachine::Stopping { .. } => Some(InstallBlockReason::Stopping),
        AppStateMachine::Downloading { .. } => Some(InstallBlockReason::Downloading),
        AppStateMachine::Loading { .. } => Some(InstallBlockReason::Loading),
        AppStateMachine::Unloading { .. } => Some(InstallBlockReason::Unloading),
        AppStateMachine::Idle
        | AppStateMachine::Downloaded { .. }
        | AppStateMachine::Ready { .. }
        | AppStateMachine::Error { .. } => None,
    }
}

fn with_state<R>(f: impl FnOnce(&mut DownloadState) -> R) -> Result<R, String> {
    let mut guard = DOWNLOAD
        .lock()
        .map_err(|_| "Update download state lock poisoned".to_string())?;
    Ok(f(&mut guard))
}

fn attempt_is_current(attempt: u64) -> bool {
    DOWNLOAD_ATTEMPT.load(Ordering::SeqCst) == attempt
}

/// Snapshot of the in-flight / ready update download. Source of truth for the UI.
pub fn get_update_download_status() -> Result<UpdateDownloadStatus, String> {
    with_state(|s| s.status())
}

/// Why install is currently refused, if it is. Pure read — no side effects.
pub fn get_update_install_block(
    state: Arc<AppState>,
) -> Result<Option<InstallBlockReason>, String> {
    let machine = state.current_machine_state()?;
    Ok(install_blocked_reason(&machine))
}

/// Start downloading the updater artifact. Detection has already happened via
/// `update_check`; this fetches the signed manifest, downloads, and verifies.
pub async fn download_update() -> Result<UpdateDownloadStatus, String> {
    let attempt = {
        let busy =
            with_state(|s| matches!(s.phase, UpdatePhase::Downloading | UpdatePhase::Ready))?;
        if busy {
            return with_state(|s| s.status());
        }
        CANCEL_DOWNLOAD.store(false, Ordering::SeqCst);
        let attempt = DOWNLOAD_ATTEMPT.fetch_add(1, Ordering::SeqCst) + 1;
        with_state(|s| {
            *s = DownloadState {
                phase: UpdatePhase::Downloading,
                ..DownloadState::default()
            };
        })?;
        attempt
    };

    let result = crate::native::updater::download_and_verify(move |downloaded, total| {
        if !attempt_is_current(attempt) {
            return;
        }
        let _ = with_state(|s| {
            if s.phase == UpdatePhase::Downloading {
                s.downloaded_bytes = downloaded;
                if let Some(total) = total {
                    s.total_bytes = Some(total);
                }
            }
        });
    })
    .await;

    if CANCEL_DOWNLOAD.load(Ordering::SeqCst) || !attempt_is_current(attempt) {
        return with_state(|s| s.status());
    }

    match result {
        Ok(ready) => {
            info!(
                version = %ready.version,
                bytes = ready.bytes.len(),
                "Update package downloaded and verified"
            );
            with_state(|s| {
                if !attempt_is_current(attempt) || s.phase != UpdatePhase::Downloading {
                    return s.status();
                }
                s.phase = UpdatePhase::Ready;
                s.version = Some(ready.version.clone());
                s.downloaded_bytes = ready.bytes.len() as u64;
                s.total_bytes = Some(ready.bytes.len() as u64);
                s.error = None;
                s.manual_fallback = false;
                s.ready = Some(ready);
                s.status()
            })
        }
        Err(e) => fail(format!("Update download failed: {e}"), true),
    }
}

/// Abandon an in-flight download. No-op if nothing is downloading.
pub fn cancel_update_download() -> Result<UpdateDownloadStatus, String> {
    CANCEL_DOWNLOAD.store(true, Ordering::SeqCst);
    DOWNLOAD_ATTEMPT.fetch_add(1, Ordering::SeqCst);
    with_state(|s| {
        if matches!(s.phase, UpdatePhase::Downloading) {
            *s = DownloadState::default();
        }
        s.status()
    })
}

/// Replace the on-disk bundle and restart. Refuses before any side effect when
/// the state machine is busy.
pub async fn install_update(state: Arc<AppState>) -> Result<(), String> {
    let machine = state.current_machine_state()?;
    if let Some(reason) = install_blocked_reason(&machine) {
        return Err(format!(
            "Cannot install update while {}",
            reason.as_str().replace('_', " ")
        ));
    }

    let package = with_state(|s| {
        if s.phase != UpdatePhase::Ready {
            return Err("No update package is ready to install".to_string());
        }
        s.ready
            .take()
            .ok_or_else(|| "No update package is ready to install".to_string())
    })??;

    // Explicit shutdown before install: free Metal before the process exits
    // as part of the relaunch, same requirement `RunEvent::ExitRequested`
    // enforced before this ticket.
    if let Err(e) = state.engine_actor.shutdown() {
        warn!("Engine shutdown before update restart: {e}");
    }

    info!(version = %package.version, "Installing update; relaunching");
    // Never returns on success — the process exits as part of the relaunch.
    crate::native::updater::install_and_relaunch(&package.bytes)
}

fn fail(error: String, manual_fallback: bool) -> Result<UpdateDownloadStatus, String> {
    with_state(|s| {
        let version = s.version.clone();
        *s = DownloadState {
            phase: UpdatePhase::Failed,
            version,
            error: Some(error),
            manual_fallback,
            ..DownloadState::default()
        };
        s.status()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::TranscriptionProfile;
    use crate::state_machine::{AppStateMachine, ErrorRecovery};

    fn profile() -> TranscriptionProfile {
        TranscriptionProfile {
            engine_id: "test".into(),
            engine_label: "Test".into(),
            model_id: "m".into(),
            model_label: "M".into(),
            backend_id: "b".into(),
            backend_label: "B".into(),
        }
    }

    #[test]
    fn install_is_blocked_in_every_busy_state() {
        let p = profile();
        let cases = [
            (
                AppStateMachine::RecordingDictation {
                    profile: p.clone(),
                    session_id: 1,
                },
                InstallBlockReason::RecordingDictation,
            ),
            (
                AppStateMachine::RecordingMeeting {
                    profile: p.clone(),
                    session_id: 1,
                    meeting_id: "m1".into(),
                },
                InstallBlockReason::RecordingMeeting,
            ),
            (
                AppStateMachine::Stopping {
                    profile: p.clone(),
                    was_recording: crate::state_machine::RecordingKind::Dictation,
                },
                InstallBlockReason::Stopping,
            ),
            (
                AppStateMachine::Downloading { profile: p.clone() },
                InstallBlockReason::Downloading,
            ),
            (
                AppStateMachine::Loading { profile: p.clone() },
                InstallBlockReason::Loading,
            ),
            (
                AppStateMachine::Unloading {
                    profile: p.clone(),
                    next_profile: None,
                },
                InstallBlockReason::Unloading,
            ),
        ];
        for (machine, expected) in cases {
            assert_eq!(install_blocked_reason(&machine), Some(expected));
        }
    }

    #[test]
    fn install_block_reason_wire_encoding_matches_as_str() {
        for reason in [
            InstallBlockReason::RecordingDictation,
            InstallBlockReason::RecordingMeeting,
            InstallBlockReason::Stopping,
            InstallBlockReason::Downloading,
            InstallBlockReason::Loading,
            InstallBlockReason::Unloading,
        ] {
            let json = serde_json::to_string(&reason).unwrap();
            assert_eq!(json, format!("\"{}\"", reason.as_str()));
            assert_eq!(
                serde_json::from_str::<InstallBlockReason>(&json).unwrap(),
                reason
            );
        }
    }

    #[test]
    fn install_is_allowed_when_idle_ready_or_error() {
        let p = profile();
        assert!(install_blocked_reason(&AppStateMachine::Idle).is_none());
        assert!(install_blocked_reason(&AppStateMachine::Ready { profile: p.clone() }).is_none());
        assert!(
            install_blocked_reason(&AppStateMachine::Downloaded { profile: p.clone() }).is_none()
        );
        assert!(
            install_blocked_reason(&AppStateMachine::Error {
                message: "x".into(),
                recovery: ErrorRecovery::RetryFromIdle,
            })
            .is_none()
        );
    }

    #[test]
    fn cancel_invalidates_prior_download_attempt() {
        let before = DOWNLOAD_ATTEMPT.load(Ordering::SeqCst);
        CANCEL_DOWNLOAD.store(false, Ordering::SeqCst);
        let _ = with_state(|s| {
            *s = DownloadState {
                phase: UpdatePhase::Downloading,
                ..DownloadState::default()
            };
        });
        CANCEL_DOWNLOAD.store(true, Ordering::SeqCst);
        DOWNLOAD_ATTEMPT.fetch_add(1, Ordering::SeqCst);
        let _ = with_state(|s| {
            if matches!(s.phase, UpdatePhase::Downloading) {
                *s = DownloadState::default();
            }
        });
        assert!(CANCEL_DOWNLOAD.load(Ordering::SeqCst));
        assert_eq!(DOWNLOAD_ATTEMPT.load(Ordering::SeqCst), before + 1);
        assert!(!attempt_is_current(before));
        let phase = with_state(|s| s.phase).unwrap();
        assert_eq!(phase, UpdatePhase::Idle);
    }
}
