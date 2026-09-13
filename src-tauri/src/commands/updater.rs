//! Download and install in-app updates via `tauri-plugin-updater`.
//!
//! Detection stays in [`crate::update_check`]: this module only executes.
//! Progress and ready bytes live here so a webview reload cannot lose them.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Update, UpdaterExt};
use tauri_specta::Event;
use tracing::{info, warn};

use crate::app_events::{UpdateDownloadProgress, UpdatePhase};
use crate::state::AppState;
use crate::state_machine::AppStateMachine;

/// Why "Install and restart" must stay disabled / refuse before any side effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
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

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct UpdateDownloadStatus {
    pub phase: UpdatePhase,
    pub version: Option<String>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
    /// Offer the GitHub release-page fallback when true.
    pub manual_fallback: bool,
}

struct ReadyPackage {
    update: Update,
    bytes: Vec<u8>,
    version: String,
}

struct DownloadState {
    phase: UpdatePhase,
    version: Option<String>,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    error: Option<String>,
    manual_fallback: bool,
    ready: Option<ReadyPackage>,
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

fn emit_status(app: &AppHandle, status: UpdateDownloadStatus) {
    let _ = (UpdateDownloadProgress {
        phase: status.phase,
        version: status.version,
        downloaded_bytes: status.downloaded_bytes,
        total_bytes: status.total_bytes,
        error: status.error,
        manual_fallback: status.manual_fallback,
    })
    .emit(app);
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

fn mutate_and_emit(app: &AppHandle, f: impl FnOnce(&mut DownloadState)) -> Result<(), String> {
    let status = with_state(|s| {
        f(s);
        s.status()
    })?;
    emit_status(app, status);
    Ok(())
}

fn attempt_is_current(attempt: u64) -> bool {
    DOWNLOAD_ATTEMPT.load(Ordering::SeqCst) == attempt
}

/// Snapshot of the in-flight / ready update download. Source of truth for the UI.
#[tauri::command]
#[specta::specta]
pub fn get_update_download_status() -> Result<UpdateDownloadStatus, String> {
    with_state(|s| s.status())
}

/// Why install is currently refused, if it is. Pure read — no side effects.
#[tauri::command]
#[specta::specta]
pub fn get_update_install_block(
    state: State<'_, AppState>,
) -> Result<Option<InstallBlockReason>, String> {
    let machine = state.current_machine_state()?;
    Ok(install_blocked_reason(&machine))
}

/// Start downloading the updater artifact. Detection has already happened via
/// `update_check`; this only obtains a plugin handle and fetches bytes.
#[tauri::command]
#[specta::specta]
pub async fn download_update(app: AppHandle) -> Result<UpdateDownloadStatus, String> {
    let attempt = {
        let busy =
            with_state(|s| matches!(s.phase, UpdatePhase::Downloading | UpdatePhase::Ready))?;
        if busy {
            return with_state(|s| s.status());
        }
        CANCEL_DOWNLOAD.store(false, Ordering::SeqCst);
        let attempt = DOWNLOAD_ATTEMPT.fetch_add(1, Ordering::SeqCst) + 1;
        mutate_and_emit(&app, |s| {
            *s = DownloadState {
                phase: UpdatePhase::Downloading,
                ..DownloadState::default()
            };
        })?;
        attempt
    };

    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(e) => {
            if !attempt_is_current(attempt) {
                return with_state(|s| s.status());
            }
            return fail(&app, format!("Updater unavailable: {e}"), true);
        }
    };

    let update = match updater.check().await {
        Ok(Some(update)) => update,
        Ok(None) => {
            if !attempt_is_current(attempt) {
                return with_state(|s| s.status());
            }
            // Release exists (update_check said so) but no updater artifact yet.
            return fail(
                &app,
                "This release has no in-app update package yet. Download it from GitHub instead."
                    .into(),
                true,
            );
        }
        Err(e) => {
            if !attempt_is_current(attempt) {
                return with_state(|s| s.status());
            }
            let message = e.to_string();
            return fail(
                &app,
                format!("Could not prepare the update: {message}"),
                true,
            );
        }
    };

    if CANCEL_DOWNLOAD.load(Ordering::SeqCst) || !attempt_is_current(attempt) {
        return with_state(|s| s.status());
    }

    let version = update.version.clone();
    mutate_and_emit(&app, |s| {
        if attempt_is_current(attempt) && s.phase == UpdatePhase::Downloading {
            s.version = Some(version.clone());
        }
    })?;

    let app_progress = app.clone();
    let download_result = update
        .download(
            |chunk_len, content_length| {
                if CANCEL_DOWNLOAD.load(Ordering::SeqCst) || !attempt_is_current(attempt) {
                    return;
                }
                let status = with_state(|s| {
                    if !attempt_is_current(attempt) || s.phase != UpdatePhase::Downloading {
                        return None;
                    }
                    s.downloaded_bytes = s.downloaded_bytes.saturating_add(chunk_len as u64);
                    if content_length.is_some() {
                        s.total_bytes = content_length;
                    }
                    Some(s.status())
                })
                .ok()
                .flatten();
                if let Some(status) = status {
                    emit_status(&app_progress, status);
                }
            },
            || {},
        )
        .await;

    if CANCEL_DOWNLOAD.load(Ordering::SeqCst) || !attempt_is_current(attempt) {
        return with_state(|s| s.status());
    }

    match download_result {
        Ok(bytes) => {
            if CANCEL_DOWNLOAD.load(Ordering::SeqCst) || !attempt_is_current(attempt) {
                return with_state(|s| s.status());
            }
            info!(
                version = %version,
                bytes = bytes.len(),
                "Update package downloaded and verified"
            );
            let status = with_state(|s| {
                if !attempt_is_current(attempt) || s.phase != UpdatePhase::Downloading {
                    return s.status();
                }
                s.phase = UpdatePhase::Ready;
                s.version = Some(version.clone());
                s.downloaded_bytes = bytes.len() as u64;
                s.total_bytes = Some(bytes.len() as u64);
                s.error = None;
                s.manual_fallback = false;
                s.ready = Some(ReadyPackage {
                    update,
                    bytes,
                    version,
                });
                s.status()
            })?;
            emit_status(&app, status.clone());
            Ok(status)
        }
        Err(e) => {
            if !attempt_is_current(attempt) {
                return with_state(|s| s.status());
            }
            let message = e.to_string();
            fail(&app, format!("Update download failed: {message}"), true)
        }
    }
}

/// Abandon an in-flight download. No-op if nothing is downloading.
#[tauri::command]
#[specta::specta]
pub fn cancel_update_download(app: AppHandle) -> Result<UpdateDownloadStatus, String> {
    CANCEL_DOWNLOAD.store(true, Ordering::SeqCst);
    DOWNLOAD_ATTEMPT.fetch_add(1, Ordering::SeqCst);
    let status = with_state(|s| {
        if matches!(s.phase, UpdatePhase::Downloading) {
            *s = DownloadState::default();
        }
        s.status()
    })?;
    emit_status(&app, status.clone());
    Ok(status)
}

/// Replace the on-disk bundle and restart. Refuses before any side effect when
/// the state machine is busy.
#[tauri::command]
#[specta::specta]
pub async fn install_update(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
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

    let version = package.version.clone();
    // Install first: the running process keeps its in-memory engine until we
    // restart. Shutting down before a fallible install would leave transcription
    // dead if the write fails.
    if let Err(e) = package.update.install(&package.bytes) {
        let message = e.to_string();
        let user = format!(
            "Could not write the update (is Soufflé in /Applications writable?): {message}"
        );
        // Put the package back so a retry is possible after a write failure.
        let status = with_state(|s| {
            s.phase = UpdatePhase::Failed;
            s.version = Some(version.clone());
            s.error = Some(user.clone());
            s.manual_fallback = true;
            s.ready = Some(package);
            s.status()
        })?;
        emit_status(&app, status);
        return Err(user);
    }

    // Explicit shutdown: AppHandle::restart() on the main thread skips
    // ExitRequested (tauri 2.11), and we must free Metal before process exit.
    if let Err(e) = state.engine_actor.shutdown() {
        warn!("Engine shutdown before update restart: {e}");
    }

    info!(version = %version, "Update installed; restarting");
    app.restart();
    #[allow(unreachable_code)]
    Ok(())
}

fn fail(
    app: &AppHandle,
    error: String,
    manual_fallback: bool,
) -> Result<UpdateDownloadStatus, String> {
    let status = with_state(|s| {
        let version = s.version.clone();
        *s = DownloadState {
            phase: UpdatePhase::Failed,
            version,
            error: Some(error),
            manual_fallback,
            ..DownloadState::default()
        };
        s.status()
    })?;
    emit_status(app, status.clone());
    Ok(status)
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
        // Mimic cancel_update_download without needing an AppHandle emit.
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
