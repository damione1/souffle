//! Update dialog boundary (SOU-195). Converts the updater's closed sets
//! (`souffle_lib::app_events::UpdatePhase`, `commands::updater::
//! InstallBlockReason`) into their `types.slint` mirrors with exhaustive
//! `match`es, so the dialog never compares bare strings like `"ready"` and
//! never shows a raw `recording_dictation` token to the user.

use crate::{InstallBlockReason as SlintInstallBlockReason, MainWindow, UpdatePhase};
use souffle_lib::app_events::UpdatePhase as LibUpdatePhase;
use souffle_lib::commands::updater::InstallBlockReason;

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
}
