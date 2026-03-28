use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

use crate::state_machine::AppStateMachine;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(rename_all = "kebab-case")]
pub enum AppView {
    Transcription,
    Meeting,
    MeetingHistory,
    Settings,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct Navigate(pub AppView);

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct ShortcutToggle;

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct ShortcutPttStart;

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct ShortcutPttStop;

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct StateChanged(pub AppStateMachine);
