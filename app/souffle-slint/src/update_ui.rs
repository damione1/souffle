//! Update dialog + Settings "Mises à jour" boundary (SOU-195). Converts the
//! updater's closed sets (`souffle_lib::app_events::UpdatePhase`,
//! `commands::updater::InstallBlockReason`) into their `types.slint` mirrors
//! with exhaustive `match`es, so the dialog never compares bare strings like
//! `"ready"` and never shows a raw `recording_dictation` token to the user.
//!
//! The Settings row's flow (check -> download -> install and restart) is the
//! [`UpdateRow`] ADT. Every decision about it is a pure function here, so the
//! wiring in `main.rs` only runs side effects and the decisions are tested.

use std::cell::RefCell;

use crate::{
    InstallBlockReason as SlintInstallBlockReason, MainWindow, UpdatePhase, UpdateRowStatus,
};
use souffle_lib::app_events::UpdatePhase as LibUpdatePhase;
use souffle_lib::commands::updater::{InstallBlockReason, UpdateDownloadStatus};
use souffle_lib::update_check::UpdateCheckResult;

pub fn update_phase_to_slint(phase: LibUpdatePhase) -> UpdatePhase {
    match phase {
        LibUpdatePhase::Idle => UpdatePhase::Idle,
        LibUpdatePhase::Downloading => UpdatePhase::Downloading,
        LibUpdatePhase::Ready => UpdatePhase::Ready,
        LibUpdatePhase::Failed => UpdatePhase::Failed,
    }
}

pub fn install_block_reason_to_slint(reason: InstallBlockReason) -> SlintInstallBlockReason {
    match reason {
        InstallBlockReason::RecordingDictation => SlintInstallBlockReason::RecordingDictation,
        InstallBlockReason::RecordingMeeting => SlintInstallBlockReason::RecordingMeeting,
        InstallBlockReason::Stopping => SlintInstallBlockReason::Stopping,
        InstallBlockReason::Downloading => SlintInstallBlockReason::Downloading,
        InstallBlockReason::Loading => SlintInstallBlockReason::Loading,
        InstallBlockReason::Unloading => SlintInstallBlockReason::Unloading,
    }
}

/// Slint has no `Option`, so "install is allowed" and "blocked because X"
/// travel as a `bool` + enum pair on the Window. This is the single site
/// that writes both, so they cannot drift apart: the reason is only
/// meaningful while `update-install-blocked` is true.
pub fn project_install_block(window: &MainWindow, block: Option<InstallBlockReason>) {
    window.set_update_install_blocked(block.is_some());
    if let Some(reason) = block {
        window.set_update_install_block_reason(install_block_reason_to_slint(reason));
    }
}

/// Where Settings > "Mises à jour" is in its check -> download -> install
/// flow. Before this type existed the row could only check: it reported
/// "update available" and the same button checked again, forever (v0.16.x).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateRow {
    /// Nothing checked this session.
    Unchecked,
    Checking,
    UpToDate,
    CheckFailed {
        error: String,
    },
    Available {
        version: String,
    },
    Downloading {
        version: String,
        percent: Option<u8>,
    },
    /// Verified package in memory. `blocked` is the last install-block
    /// reading; a click re-queries it before installing.
    Ready {
        version: String,
        blocked: Option<InstallBlockReason>,
    },
    /// The user asked to install; the process is about to restart.
    Installing {
        version: String,
    },
    Failed {
        version: String,
        error: String,
    },
}

impl UpdateRow {
    /// The release this row is about, once one is known.
    pub fn version(&self) -> Option<&str> {
        match self {
            Self::Unchecked | Self::Checking | Self::UpToDate | Self::CheckFailed { .. } => None,
            Self::Available { version }
            | Self::Downloading { version, .. }
            | Self::Ready { version, .. }
            | Self::Installing { version }
            | Self::Failed { version, .. } => Some(version),
        }
    }
}

/// What a click on the row's button does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateRowClick {
    Check,
    Download,
    Install {
        version: String,
    },
    /// A check, download or install is already running.
    Ignore,
}

pub fn update_row_click(row: &UpdateRow) -> UpdateRowClick {
    match row {
        UpdateRow::Unchecked | UpdateRow::UpToDate | UpdateRow::CheckFailed { .. } => {
            UpdateRowClick::Check
        }
        UpdateRow::Available { .. } | UpdateRow::Failed { .. } => UpdateRowClick::Download,
        UpdateRow::Ready { version, .. } => UpdateRowClick::Install {
            version: version.clone(),
        },
        UpdateRow::Checking | UpdateRow::Downloading { .. } | UpdateRow::Installing { .. } => {
            UpdateRowClick::Ignore
        }
    }
}

/// Whole percent downloaded, when the server announced a size.
pub fn download_percent(downloaded: u64, total: Option<u64>) -> Option<u8> {
    let total = total.filter(|total| *total > 0)?;
    let percent = downloaded.saturating_mul(100) / total;
    Some(u8::try_from(percent.min(100)).unwrap_or(100))
}

/// An update to `version` exists; the shared download state (which the
/// Update Available dialog drives too) says how far along it already is.
fn row_for_available(version: String, download: &UpdateDownloadStatus) -> UpdateRow {
    match download.phase {
        // A fresh check after a failure is a clean slate: offer it again.
        LibUpdatePhase::Idle | LibUpdatePhase::Failed => UpdateRow::Available { version },
        LibUpdatePhase::Downloading => UpdateRow::Downloading {
            version,
            percent: download_percent(download.downloaded_bytes, download.total_bytes),
        },
        LibUpdatePhase::Ready => UpdateRow::Ready {
            version: download.version.clone().unwrap_or(version),
            blocked: None,
        },
    }
}

/// Row once a check the user asked for has finished.
pub fn update_row_from_check(
    result: Result<UpdateCheckResult, String>,
    download: &UpdateDownloadStatus,
) -> UpdateRow {
    let result = match result {
        Ok(result) => result,
        Err(error) => return UpdateRow::CheckFailed { error },
    };
    if let Some(error) = result.check_error {
        return UpdateRow::CheckFailed { error };
    }
    match result.latest_version.filter(|_| result.update_available) {
        Some(version) => row_for_available(version, download),
        None => UpdateRow::UpToDate,
    }
}

/// Row after a startup or background check announced `version`, or `None`
/// to leave it alone: a flow the user started, or an error they have not
/// acted on yet, is not overwritten by an automatic check.
pub fn update_row_after_announcement(
    current: &UpdateRow,
    version: String,
    download: &UpdateDownloadStatus,
) -> Option<UpdateRow> {
    match current {
        UpdateRow::Unchecked
        | UpdateRow::UpToDate
        | UpdateRow::CheckFailed { .. }
        | UpdateRow::Available { .. } => Some(row_for_available(version, download)),
        UpdateRow::Checking
        | UpdateRow::Downloading { .. }
        | UpdateRow::Ready { .. }
        | UpdateRow::Installing { .. }
        | UpdateRow::Failed { .. } => None,
    }
}

/// Fresh progress for a row that is still downloading, or `None` when the
/// row moved on (or the download settled, which the download's own result
/// projects).
pub fn update_row_with_progress(
    current: &UpdateRow,
    download: &UpdateDownloadStatus,
) -> Option<UpdateRow> {
    let version = match current {
        UpdateRow::Downloading { version, .. } => version.clone(),
        UpdateRow::Unchecked
        | UpdateRow::Checking
        | UpdateRow::UpToDate
        | UpdateRow::CheckFailed { .. }
        | UpdateRow::Available { .. }
        | UpdateRow::Ready { .. }
        | UpdateRow::Installing { .. }
        | UpdateRow::Failed { .. } => return None,
    };
    match download.phase {
        LibUpdatePhase::Downloading => Some(UpdateRow::Downloading {
            version,
            percent: download_percent(download.downloaded_bytes, download.total_bytes),
        }),
        LibUpdatePhase::Idle | LibUpdatePhase::Ready | LibUpdatePhase::Failed => None,
    }
}

/// Row once `download_update` (or `install_update`) settled, from the
/// updater's own status - or the error when the command could not run.
/// `blocked` is the install block read right after; it only matters for a
/// ready package.
pub fn update_row_from_download(
    version: String,
    result: Result<&UpdateDownloadStatus, &str>,
    blocked: Option<InstallBlockReason>,
) -> UpdateRow {
    let status = match result {
        Ok(status) => status,
        Err(error) => {
            return UpdateRow::Failed {
                version,
                error: error.to_string(),
            };
        }
    };
    match status.phase {
        // Cancelled: back to offering it.
        LibUpdatePhase::Idle => UpdateRow::Available { version },
        LibUpdatePhase::Downloading => UpdateRow::Downloading {
            version,
            percent: download_percent(status.downloaded_bytes, status.total_bytes),
        },
        LibUpdatePhase::Ready => UpdateRow::Ready {
            version: status.version.clone().unwrap_or(version),
            blocked,
        },
        LibUpdatePhase::Failed => UpdateRow::Failed {
            version,
            error: status.error.clone().unwrap_or_default(),
        },
    }
}

/// Whether a finished download from the row installs straight away. The
/// user clicked "install" on the row, so a ready, unblocked package does not
/// need a second click; a blocked one waits for that click.
pub fn should_auto_install(row: &UpdateRow) -> bool {
    match row {
        UpdateRow::Ready { blocked, .. } => blocked.is_none(),
        UpdateRow::Unchecked
        | UpdateRow::Checking
        | UpdateRow::UpToDate
        | UpdateRow::CheckFailed { .. }
        | UpdateRow::Available { .. }
        | UpdateRow::Downloading { .. }
        | UpdateRow::Installing { .. }
        | UpdateRow::Failed { .. } => false,
    }
}

/// The one writer of the row's Window properties. Payload properties are
/// written for every status (empty when absent) so a stale version or error
/// can never leak into the next status's copy.
pub fn project_update_row(window: &MainWindow, row: &UpdateRow) {
    let none = String::new();
    let (status, version, error, percent, blocked) = match row {
        UpdateRow::Unchecked => (UpdateRowStatus::Unchecked, &none, &none, None, None),
        UpdateRow::Checking => (UpdateRowStatus::Checking, &none, &none, None, None),
        UpdateRow::UpToDate => (UpdateRowStatus::UpToDate, &none, &none, None, None),
        UpdateRow::CheckFailed { error } => {
            (UpdateRowStatus::CheckFailed, &none, error, None, None)
        }
        UpdateRow::Available { version } => {
            (UpdateRowStatus::Available, version, &none, None, None)
        }
        UpdateRow::Downloading { version, percent } => {
            (UpdateRowStatus::Downloading, version, &none, *percent, None)
        }
        UpdateRow::Ready { version, blocked } => {
            (UpdateRowStatus::Ready, version, &none, None, *blocked)
        }
        UpdateRow::Installing { version } => {
            (UpdateRowStatus::Installing, version, &none, None, None)
        }
        UpdateRow::Failed { version, error } => {
            (UpdateRowStatus::Failed, version, error, None, None)
        }
    };
    window.set_settings_update_row(status);
    window.set_settings_update_version(version.as_str().into());
    window.set_settings_update_error(error.as_str().into());
    window.set_settings_update_has_progress(percent.is_some());
    window.set_settings_update_progress(percent.map_or(0, i32::from));
    window.set_settings_update_blocked(blocked.is_some());
    if let Some(reason) = blocked {
        window.set_settings_update_block_reason(install_block_reason_to_slint(reason));
    }
}

thread_local! {
    // UI-thread slot for the row, so `dispatch_native_action` (background
    // check) and the Settings / dialog callbacks read the same value.
    static UPDATE_ROW: RefCell<UpdateRow> = const { RefCell::new(UpdateRow::Unchecked) };
}

pub fn current_update_row() -> UpdateRow {
    UPDATE_ROW.with(|slot| slot.borrow().clone())
}

/// Store and project `row`. The only way the row changes.
pub fn set_update_row(window: &MainWindow, row: UpdateRow) {
    project_update_row(window, &row);
    UPDATE_ROW.with(|slot| *slot.borrow_mut() = row);
}

/// A startup or background check found `version`: surface it on the row
/// too, so Settings offers the install without another check.
pub fn announce_update(window: &MainWindow, version: String) {
    let Ok(download) = souffle_lib::commands::get_update_download_status() else {
        return;
    };
    if let Some(row) = update_row_after_announcement(&current_update_row(), version, &download) {
        set_update_row(window, row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_phase_maps_every_variant_distinctly() {
        let mapped = [
            update_phase_to_slint(LibUpdatePhase::Idle),
            update_phase_to_slint(LibUpdatePhase::Downloading),
            update_phase_to_slint(LibUpdatePhase::Ready),
            update_phase_to_slint(LibUpdatePhase::Failed),
        ];
        assert_eq!(mapped[0], UpdatePhase::Idle);
        assert_eq!(mapped[1], UpdatePhase::Downloading);
        assert_eq!(mapped[2], UpdatePhase::Ready);
        assert_eq!(mapped[3], UpdatePhase::Failed);
    }

    #[test]
    fn install_block_reason_maps_every_variant_distinctly() {
        let variants = [
            InstallBlockReason::RecordingDictation,
            InstallBlockReason::RecordingMeeting,
            InstallBlockReason::Stopping,
            InstallBlockReason::Downloading,
            InstallBlockReason::Loading,
            InstallBlockReason::Unloading,
        ];
        let mapped: Vec<SlintInstallBlockReason> = variants
            .iter()
            .map(|&reason| install_block_reason_to_slint(reason))
            .collect();
        for (i, a) in mapped.iter().enumerate() {
            for (j, b) in mapped.iter().enumerate() {
                assert_eq!(i == j, a == b, "{:?} vs {:?}", variants[i], variants[j]);
            }
        }
    }

    fn status(phase: LibUpdatePhase) -> UpdateDownloadStatus {
        UpdateDownloadStatus {
            phase,
            version: None,
            downloaded_bytes: 0,
            total_bytes: None,
            error: None,
            manual_fallback: false,
        }
    }

    fn check(available: bool, latest: Option<&str>, error: Option<&str>) -> UpdateCheckResult {
        UpdateCheckResult {
            current_version: "0.16.1".into(),
            latest_version: latest.map(str::to_owned),
            update_available: available,
            release_notes: None,
            release_url: None,
            check_error: error.map(str::to_owned),
        }
    }

    fn available() -> UpdateRow {
        UpdateRow::Available {
            version: "0.16.2".into(),
        }
    }

    /// The v0.16.1 bug: a found update left the button checking again.
    #[test]
    fn a_click_on_an_available_update_downloads_it_instead_of_checking_again() {
        assert_eq!(update_row_click(&available()), UpdateRowClick::Download);
    }

    #[test]
    fn each_row_click_does_the_next_step_of_the_flow() {
        let v = || "0.16.2".to_string();
        assert_eq!(
            update_row_click(&UpdateRow::Unchecked),
            UpdateRowClick::Check
        );
        assert_eq!(
            update_row_click(&UpdateRow::UpToDate),
            UpdateRowClick::Check
        );
        assert_eq!(
            update_row_click(&UpdateRow::CheckFailed { error: "x".into() }),
            UpdateRowClick::Check
        );
        assert_eq!(
            update_row_click(&UpdateRow::Failed {
                version: v(),
                error: "x".into()
            }),
            UpdateRowClick::Download
        );
        assert_eq!(
            update_row_click(&UpdateRow::Ready {
                version: v(),
                blocked: Some(InstallBlockReason::RecordingMeeting)
            }),
            UpdateRowClick::Install { version: v() }
        );
        for busy in [
            UpdateRow::Checking,
            UpdateRow::Downloading {
                version: v(),
                percent: Some(10),
            },
            UpdateRow::Installing { version: v() },
        ] {
            assert_eq!(update_row_click(&busy), UpdateRowClick::Ignore, "{busy:?}");
        }
    }

    #[test]
    fn a_check_result_maps_to_up_to_date_available_or_failed() {
        let idle = status(LibUpdatePhase::Idle);
        assert_eq!(
            update_row_from_check(Ok(check(true, Some("0.16.2"), None)), &idle),
            available()
        );
        assert_eq!(
            update_row_from_check(Ok(check(false, Some("0.16.1"), None)), &idle),
            UpdateRow::UpToDate
        );
        assert_eq!(
            update_row_from_check(
                Ok(check(false, None, Some("GitHub returned HTTP 403"))),
                &idle
            ),
            UpdateRow::CheckFailed {
                error: "GitHub returned HTTP 403".into()
            }
        );
        assert_eq!(
            update_row_from_check(Err("join".into()), &idle),
            UpdateRow::CheckFailed {
                error: "join".into()
            }
        );
    }

    #[test]
    fn a_check_does_not_forget_a_download_the_dialog_already_finished() {
        let mut ready = status(LibUpdatePhase::Ready);
        ready.version = Some("0.16.2".into());
        assert_eq!(
            update_row_from_check(Ok(check(true, Some("0.16.2"), None)), &ready),
            UpdateRow::Ready {
                version: "0.16.2".into(),
                blocked: None
            }
        );
        let failed = status(LibUpdatePhase::Failed);
        assert_eq!(
            update_row_from_check(Ok(check(true, Some("0.16.2"), None)), &failed),
            available()
        );
    }

    #[test]
    fn download_percent_needs_a_known_nonzero_size_and_never_exceeds_100() {
        assert_eq!(download_percent(10, None), None);
        assert_eq!(download_percent(10, Some(0)), None);
        assert_eq!(download_percent(0, Some(200)), Some(0));
        assert_eq!(download_percent(50, Some(200)), Some(25));
        assert_eq!(download_percent(300, Some(200)), Some(100));
        assert_eq!(download_percent(u64::MAX, Some(1)), Some(100));
    }

    #[test]
    fn progress_only_updates_a_row_that_is_still_downloading() {
        let mut downloading = status(LibUpdatePhase::Downloading);
        downloading.downloaded_bytes = 7;
        downloading.total_bytes = Some(28);
        let row = UpdateRow::Downloading {
            version: "0.16.2".into(),
            percent: None,
        };
        assert_eq!(
            update_row_with_progress(&row, &downloading),
            Some(UpdateRow::Downloading {
                version: "0.16.2".into(),
                percent: Some(25)
            })
        );
        assert_eq!(update_row_with_progress(&available(), &downloading), None);
        assert_eq!(
            update_row_with_progress(&row, &status(LibUpdatePhase::Ready)),
            None
        );
    }

    #[test]
    fn a_settled_download_maps_to_ready_failed_or_available() {
        let v = || "0.16.2".to_string();
        let mut ready = status(LibUpdatePhase::Ready);
        ready.version = Some("0.16.2".into());
        assert_eq!(
            update_row_from_download(v(), Ok(&ready), None),
            UpdateRow::Ready {
                version: v(),
                blocked: None
            }
        );
        assert_eq!(
            update_row_from_download(
                v(),
                Ok(&ready),
                Some(InstallBlockReason::RecordingDictation)
            ),
            UpdateRow::Ready {
                version: v(),
                blocked: Some(InstallBlockReason::RecordingDictation)
            }
        );
        let mut failed = status(LibUpdatePhase::Failed);
        failed.error = Some("Update download returned HTTP 404 Not Found".into());
        assert_eq!(
            update_row_from_download(v(), Ok(&failed), None),
            UpdateRow::Failed {
                version: v(),
                error: "Update download returned HTTP 404 Not Found".into()
            }
        );
        assert_eq!(
            update_row_from_download(v(), Ok(&status(LibUpdatePhase::Idle)), None),
            available()
        );
        assert_eq!(
            update_row_from_download(v(), Err("Join download_update task"), None),
            UpdateRow::Failed {
                version: v(),
                error: "Join download_update task".into()
            }
        );
    }

    #[test]
    fn only_a_ready_unblocked_package_installs_without_a_second_click() {
        let v = || "0.16.2".to_string();
        assert!(should_auto_install(&UpdateRow::Ready {
            version: v(),
            blocked: None
        }));
        assert!(!should_auto_install(&UpdateRow::Ready {
            version: v(),
            blocked: Some(InstallBlockReason::Loading)
        }));
        assert!(!should_auto_install(&UpdateRow::Failed {
            version: v(),
            error: "x".into()
        }));
        assert!(!should_auto_install(&available()));
    }

    #[test]
    fn an_automatic_check_never_overrides_a_flow_the_user_started() {
        let idle = status(LibUpdatePhase::Idle);
        assert_eq!(
            update_row_after_announcement(&UpdateRow::Unchecked, "0.16.2".into(), &idle),
            Some(available())
        );
        assert_eq!(
            update_row_after_announcement(&UpdateRow::UpToDate, "0.16.2".into(), &idle),
            Some(available())
        );
        for started in [
            UpdateRow::Checking,
            UpdateRow::Downloading {
                version: "0.16.2".into(),
                percent: None,
            },
            UpdateRow::Installing {
                version: "0.16.2".into(),
            },
            UpdateRow::Failed {
                version: "0.16.2".into(),
                error: "x".into(),
            },
        ] {
            assert_eq!(
                update_row_after_announcement(&started, "0.16.2".into(), &idle),
                None,
                "{started:?}"
            );
        }
    }

    #[test]
    fn version_is_known_once_an_update_was_found() {
        assert_eq!(UpdateRow::Unchecked.version(), None);
        assert_eq!(UpdateRow::Checking.version(), None);
        assert_eq!(available().version(), Some("0.16.2"));
    }
}
