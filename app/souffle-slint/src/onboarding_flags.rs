//! Port of `features/onboarding/setup.ts`. The Svelte version keys its two
//! flags off `localStorage`, which does not exist in this headless shell -
//! two marker files under the real app data directory stand in for it
//! (plain presence/absence, same semantics as the original's try/catch
//! read/write). Not part of `AppSettings`: this is local wizard-visibility
//! state, not a synced setting, matching the original's own scoping choice.

use crate::OnboardingStep;
use souffle_lib::engine::TranscriptionRuntimePhase;
use std::path::PathBuf;

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

/// First-run onboarding owns its initial model choice. Once setup is complete,
/// startup restores the persisted selection (including an interrupted download)
/// without reopening the wizard or requiring another selection in Settings.
pub fn decide_show_setup_wizard(phase: TranscriptionRuntimePhase, flags: SetupFlags) -> bool {
    if flags.setup_done {
        return false;
    }
    match phase {
        TranscriptionRuntimePhase::DownloadRequired | TranscriptionRuntimePhase::Failed => true,
        TranscriptionRuntimePhase::Downloading
        | TranscriptionRuntimePhase::LoadRequired
        | TranscriptionRuntimePhase::Loading
        | TranscriptionRuntimePhase::Ready
        | TranscriptionRuntimePhase::Unloading => !flags.permissions_done,
    }
}

/// SOU-036: what `autostart_enabled` should be once the wizard finishes. A
/// fresh install leaves the wizard with the login item registered; a
/// recovery run (model re-download on an already set-up install) keeps
/// whatever is stored, because an absent key on an existing install must
/// not turn into "on" without a gesture from the user.
pub fn decide_autostart_on_finish(recovery_only: bool, current: bool) -> bool {
    if recovery_only { current } else { true }
}

/// Which wizard pages this install still needs, in order. Never empty: a
/// fully set-up install re-entering the wizard (model recovery) gets the
/// model step alone.
pub fn wizard_steps(flags: SetupFlags) -> Vec<OnboardingStep> {
    if flags.setup_done {
        return vec![OnboardingStep::Model];
    }
    let mut steps = Vec::new();
    if !flags.permissions_done {
        steps.push(OnboardingStep::Permissions);
    }
    steps.push(OnboardingStep::Microphone);
    steps.push(OnboardingStep::Model);
    steps.push(OnboardingStep::Shortcut);
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wizard_steps_skips_permissions_when_already_done() {
        let flags = SetupFlags {
            permissions_done: true,
            setup_done: false,
        };
        assert_eq!(
            wizard_steps(flags),
            vec![
                OnboardingStep::Microphone,
                OnboardingStep::Model,
                OnboardingStep::Shortcut
            ]
        );
    }

    #[test]
    fn wizard_steps_is_model_only_once_fully_set_up() {
        let flags = SetupFlags {
            permissions_done: true,
            setup_done: true,
        };
        assert_eq!(wizard_steps(flags), vec![OnboardingStep::Model]);
    }

    #[test]
    fn wizard_steps_starts_with_permissions_on_a_fresh_install() {
        let flags = SetupFlags::default();
        assert_eq!(
            wizard_steps(flags),
            vec![
                OnboardingStep::Permissions,
                OnboardingStep::Microphone,
                OnboardingStep::Model,
                OnboardingStep::Shortcut
            ]
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
        ));
    }

    #[test]
    fn configured_install_restores_missing_model_without_reopening_setup() {
        let flags = SetupFlags {
            permissions_done: true,
            setup_done: true,
        };
        assert!(!decide_show_setup_wizard(
            TranscriptionRuntimePhase::DownloadRequired,
            flags,
        ));
    }

    #[test]
    fn does_not_reopen_over_an_in_flight_download_sou073() {
        let flags = SetupFlags {
            permissions_done: true,
            setup_done: true,
        };
        assert!(!decide_show_setup_wizard(
            TranscriptionRuntimePhase::Downloading,
            flags,
        ));
    }

    #[test]
    fn shows_wizard_on_a_fresh_install() {
        assert!(decide_show_setup_wizard(
            TranscriptionRuntimePhase::DownloadRequired,
            SetupFlags::default(),
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
