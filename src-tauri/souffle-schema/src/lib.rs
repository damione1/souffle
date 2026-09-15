//! What the Souffle app and its read-only MCP sidecar must agree on.
//!
//! The sidecar is a separate binary that opens the same SQLite file and
//! depends on neither `souffle` nor `tauri`, so anything it shares with the
//! app used to be restated on its side. This crate is the shared declaration:
//! it carries no logic, only the schema version and the row shapes both
//! processes read.

use serde::{Deserialize, Serialize};

/// Schema version 16: `meetings.system_audio` holds what the system-audio
/// leg did over the recording, so a mic-only meeting still says why.
///
/// Both the writer (`db::schema` in the app) and the reader (`souffle-mcp`)
/// build against this number. The sidecar refuses to serve a database whose
/// stored version is higher, because the columns it reads may have moved.
pub const SCHEMA_VERSION: i64 = 16;

/// Allowed values for `model_unload_timeout_minutes`: 0 (never) plus the
/// options offered in the settings UI.
pub const ALLOWED_UNLOAD_TIMEOUT_MINUTES: [u32; 4] = [0, 5, 15, 60];

pub const MEETING_AUTOSTOP_MINUTES_MIN: u32 = 3;
pub const MEETING_AUTOSTOP_MINUTES_MAX: u32 = 60;

pub const MEETING_MAX_DURATION_MINUTES_MIN: u32 = 60;
pub const MEETING_MAX_DURATION_MINUTES_MAX: u32 = 720;

/// The discrete steps the settings UI offers inside the two ranges above.
pub const MEETING_AUTOSTOP_MINUTES_OPTIONS: [u32; 4] = [5, 10, 15, 30];
pub const MEETING_MAX_DURATION_MINUTES_OPTIONS: [u32; 3] = [120, 240, 480];

/// A single action item extracted from a meeting summary pass.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, specta::Type, schemars::JsonSchema)]
pub struct StructuredActionItem {
    pub text: String,
    pub owner: Option<String>,
}

/// Typed structured summary: decisions, action items, open questions.
#[derive(
    Debug, Clone, Serialize, Deserialize, PartialEq, specta::Type, schemars::JsonSchema, Default,
)]
pub struct StructuredSummary {
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub action_items: Vec<StructuredActionItem>,
    #[serde(default)]
    pub open_questions: Vec<String>,
}
