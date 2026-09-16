//! Port of `features/onboarding/setup.ts`. The Svelte version keys its two
//! flags off `localStorage`, which does not exist in this headless shell -
//! two marker files under the real app data directory stand in for it
//! (plain presence/absence, same semantics as the original's try/catch
//! read/write). Not part of `AppSettings`: this is local wizard-visibility
//! state, not a synced setting, matching the original's own scoping choice.

use souffle_lib::engine::TranscriptionRuntimePhase;
use souffle_lib::state_machine::AppStateMachine;
use std::path::PathBuf;

pub type SetupStep = &'static str; // "permissions" | "microphone" | "model" | "shortcut"

#[derive(Debug, Clone, Copy, Default)]
pub struct SetupFlags {
    pub permissions_done: bool,
    pub setup_done: bool,
}

fn permissions_marker() -> PathBuf {
    souffle_lib::constants::app_data_dir().join(".permissions_onboarded")
}

fn setup_marker() -> PathBuf {
    souffle_lib::constants::app_data_dir().join(".setup_onboarded")
}

pub fn read_setup_flags() -> SetupFlags {
    SetupFlags {
        permissions_done: permissions_marker().exists(),
        setup_done: setup_marker().exists(),
    }
}

fn write_marker(path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, b"1");
}

pub fn mark_permissions_done() {
    write_marker(&permissions_marker());
}

pub fn mark_setup_complete() {
    write_marker(&permissions_marker());
    write_marker(&setup_marker());
}

/// SOU-073: a webview/process reload mid-download reports
/// `download_required` too, and must not reopen the wizard over a download
/// that is about to finish.
pub fn decide_show_setup_wizard(
    phase: TranscriptionRuntimePhase,
    flags: SetupFlags,
    machine_state: &AppStateMachine,
) -> bool {
    if flags.setup_done {
        return phase == TranscriptionRuntimePhase::DownloadRequired
            && !matches!(machine_state, AppStateMachine::Downloading { .. });
    }
    if flags.permissions_done && phase != TranscriptionRuntimePhase::DownloadRequired {
        return false;
    }
    true
}

/// SOU-036: what `autostart_enabled` should be once the wizard finishes. A
/// fresh install leaves the wizard with the login item registered; a
/// recovery run (model re-download on an already set-up install) keeps
/// whatever is stored, because an absent key on an existing install must
/// not turn into "on" without a gesture from the user.
pub fn decide_autostart_on_finish(recovery_only: bool, current: bool) -> bool {
    if recovery_only { current } else { true }
}

pub fn wizard_steps(flags: SetupFlags) -> Vec<SetupStep> {
    if flags.setup_done {
        return vec!["model"];
    }
    let mut steps = Vec::new();
    if !flags.permissions_done {
        steps.push("permissions");
    }
    steps.push("microphone");
    steps.push("model");
    steps.push("shortcut");
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_machine() -> AppStateMachine {
        AppStateMachine::Idle
    }

    #[test]
    fn wizard_steps_skips_permissions_when_already_done() {
        let flags = SetupFlags {
            permissions_done: true,
            setup_done: false,
        };
        assert_eq!(wizard_steps(flags), vec!["microphone", "model", "shortcut"]);
    }

    #[test]
    fn wizard_steps_is_model_only_once_fully_set_up() {
        let flags = SetupFlags {
            permissions_done: true,
            setup_done: true,
        };
        assert_eq!(wizard_steps(flags), vec!["model"]);
    }

    #[test]
    fn wizard_steps_starts_with_permissions_on_a_fresh_install() {
        let flags = SetupFlags::default();
        assert_eq!(
            wizard_steps(flags),
            vec!["permissions", "microphone", "model", "shortcut"]
        );
    }

    #[test]
    fn does_not_show_wizard_once_fully_set_up_and_model_ready() {
        let flags = SetupFlags {
            permissions_done: true,
            setup_done: true,
        };
        assert!(!decide_show_setup_wizard(
            TranscriptionRuntimePhase::Ready,
            flags,
            &ready_machine()
        ));
    }

    #[test]
    fn reopens_for_model_recovery_when_download_required_and_not_already_downloading() {
        let flags = SetupFlags {
            permissions_done: true,
            setup_done: true,
        };
        assert!(decide_show_setup_wizard(
            TranscriptionRuntimePhase::DownloadRequired,
            flags,
            &ready_machine()
        ));
    }

    #[test]
    fn does_not_reopen_over_an_in_flight_download_sou073() {
        let flags = SetupFlags {
            permissions_done: true,
            setup_done: true,
        };
        let downloading = AppStateMachine::Downloading {
            profile: Default::default(),
        };
        assert!(!decide_show_setup_wizard(
            TranscriptionRuntimePhase::DownloadRequired,
            flags,
            &downloading
        ));
    }

    #[test]
    fn shows_wizard_on_a_fresh_install() {
        assert!(decide_show_setup_wizard(
            TranscriptionRuntimePhase::DownloadRequired,
            SetupFlags::default(),
            &ready_machine()
        ));
    }

    #[test]
    fn autostart_on_finish_forces_on_for_a_fresh_install() {
        assert!(decide_autostart_on_finish(false, false));
    }

    #[test]
    fn autostart_on_finish_keeps_current_value_during_recovery() {
        assert!(!decide_autostart_on_finish(true, false));
        assert!(decide_autostart_on_finish(true, true));
    }
}
