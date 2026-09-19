//! Onboarding wizard (SOU-190). Row/step formatting only - the actual TCC
//! polling and model/shortcut orchestration live in `main.rs`'s
//! `wire_onboarding_callbacks`, reusing the same commands
//! (`get_permission_status`, `get_model_status`/`download_model`/
//! `load_model`, `save_shortcuts`) the Settings tab and the recording flow
//! already call.
//!
//! Simplified versus `PermissionsStep.svelte`: the "stale TCC entry"
//! diagnosis (tracked via a blur/focus attempt-count pair) is not
//! reproduced - Accessibility always shows the plain denied hint, and
//! "Repair" is always available rather than gated behind two failed
//! attempts. The repair action itself is real; only that one UX nicety
//! is dropped.

use crate::{MainWindow, PermissionRow, settings_ui};
use souffle_lib::permissions::{PermState, PermissionKind, PermissionStatus};

struct RowSpec {
    kind: PermissionKind,
    label: &'static str,
    description: &'static str,
    action_label: &'static str,
}

const ROWS: [RowSpec; 3] = [
    RowSpec {
        kind: PermissionKind::Microphone,
        label: "Microphone",
        description: "Nécessaire pour la dictée et les réunions.",
        action_label: "Autoriser",
    },
    RowSpec {
        kind: PermissionKind::SystemAudio,
        label: "Audio système",
        description: "Capture l'autre côté d'un appel dans les réunions.",
        action_label: "Autoriser",
    },
    RowSpec {
        kind: PermissionKind::Accessibility,
        label: "Accessibilité",
        description: "Nécessaire pour le collage automatique et certains raccourcis.",
        action_label: "Ouvrir les Réglages",
    },
];

fn state_of(status: &PermissionStatus, kind: PermissionKind) -> PermState {
    match kind {
        PermissionKind::Microphone => status.microphone,
        PermissionKind::SystemAudio => status.system_audio,
        PermissionKind::Accessibility => status.accessibility,
        PermissionKind::Calendar => status.calendar,
    }
}

fn hint_for(kind: PermissionKind, state: PermState) -> (&'static str, &'static str) {
    match (kind, state) {
        (PermissionKind::Accessibility, PermState::Denied) => (
            "Refusé. Ouvrez Réglages Système > Confidentialité et sécurité > Accessibilité.",
            "Réglages Système",
        ),
        (PermissionKind::Microphone, PermState::Denied) => (
            "Refusé. Ouvrez Réglages Système pour l'autoriser.",
            "Réglages Système",
        ),
        (PermissionKind::Microphone, PermState::NoDevice) => ("Aucun microphone détecté.", ""),
        (
            PermissionKind::Microphone,
            PermState::Granted | PermState::Unknown | PermState::Unsupported,
        )
        | (
            PermissionKind::SystemAudio | PermissionKind::Calendar,
            PermState::Granted
            | PermState::Denied
            | PermState::Unknown
            | PermState::Unsupported
            | PermState::NoDevice,
        )
        | (
            PermissionKind::Accessibility,
            PermState::Granted | PermState::Unknown | PermState::Unsupported | PermState::NoDevice,
        ) => ("", ""),
    }
}

/// `busy` is parallel to the fixed 3-row order (microphone, system_audio,
/// accessibility).
pub fn populate_permission_rows(window: &MainWindow, status: &PermissionStatus, busy: [bool; 3]) {
    let rows: Vec<PermissionRow> = ROWS
        .iter()
        .enumerate()
        .map(|(i, spec)| {
            let state = state_of(status, spec.kind);
            let (hint, hint_action_label) = hint_for(spec.kind, state);
            PermissionRow {
                kind: settings_ui::permission_kind_to_slint(spec.kind),
                label: spec.label.into(),
                description: spec.description.into(),
                state: settings_ui::perm_state_to_slint(state),
                busy: busy[i],
                action_label: spec.action_label.into(),
                hint: hint.into(),
                hint_action_label: hint_action_label.into(),
            }
        })
        .collect();
    window.set_onboarding_permission_rows(std::rc::Rc::new(slint::VecModel::from(rows)).into());
}

pub fn row_index(kind: PermissionKind) -> Option<usize> {
    ROWS.iter().position(|r| r.kind == kind)
}

pub fn step_title(step: &str) -> &'static str {
    match step {
        "permissions" => "Permissions",
        "microphone" => "Choisissez votre microphone",
        "model" => "Modèle de transcription",
        "shortcut" => "Raccourci clavier",
        _ => "",
    }
}

pub fn step_subtitle(step: &str) -> &'static str {
    match step {
        "permissions" => "Soufflé a besoin de ces autorisations macOS pour fonctionner.",
        "microphone" => "Vous pourrez changer ce choix plus tard dans les Réglages.",
        "model" => "Téléchargé une seule fois, tout tourne ensuite hors ligne.",
        "shortcut" => {
            "Utilisé pour démarrer et arrêter la dictée depuis n'importe quelle application."
        }
        _ => "",
    }
}
