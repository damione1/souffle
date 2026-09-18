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

use crate::{MainWindow, PermissionRow};
use souffle_lib::permissions::{PermState, PermissionStatus};

struct RowSpec {
    kind: &'static str,
    label: &'static str,
    description: &'static str,
    action_label: &'static str,
}

const ROWS: [RowSpec; 3] = [
    RowSpec {
        kind: "microphone",
        label: "Microphone",
        description: "Nécessaire pour la dictée et les réunions.",
        action_label: "Autoriser",
    },
    RowSpec {
        kind: "system_audio",
        label: "Audio système",
        description: "Capture l'autre côté d'un appel dans les réunions.",
        action_label: "Autoriser",
    },
    RowSpec {
        kind: "accessibility",
        label: "Accessibilité",
        description: "Nécessaire pour le collage automatique et certains raccourcis.",
        action_label: "Ouvrir les Réglages",
    },
];

fn perm_state_str(state: PermState) -> &'static str {
    match state {
        PermState::Granted => "granted",
        PermState::Denied => "denied",
        PermState::Unknown => "unknown",
        PermState::Unsupported => "unsupported",
        PermState::NoDevice => "no_device",
    }
}

fn state_of(status: &PermissionStatus, kind: &str) -> PermState {
    match kind {
        "microphone" => status.microphone,
        "system_audio" => status.system_audio,
        "accessibility" => status.accessibility,
        _ => PermState::Unknown,
    }
}

fn hint_for(kind: &str, state: PermState) -> (&'static str, &'static str) {
    match (kind, state) {
        ("accessibility", PermState::Denied) => (
            "Refusé. Ouvrez Réglages Système > Confidentialité et sécurité > Accessibilité.",
            "Réglages Système",
        ),
        ("microphone", PermState::Denied) => (
            "Refusé. Ouvrez Réglages Système pour l'autoriser.",
            "Réglages Système",
        ),
        ("microphone", PermState::NoDevice) => ("Aucun microphone détecté.", ""),
        _ => ("", ""),
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
                kind: spec.kind.into(),
                label: spec.label.into(),
                description: spec.description.into(),
                state: perm_state_str(state).into(),
                busy: busy[i],
                action_label: spec.action_label.into(),
                hint: hint.into(),
                hint_action_label: hint_action_label.into(),
            }
        })
        .collect();
    window.set_onboarding_permission_rows(std::rc::Rc::new(slint::VecModel::from(rows)).into());
}

pub fn row_index(kind: &str) -> Option<usize> {
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
