//! What the Souffle app and its read-only MCP sidecar must agree on.
//!
//! The sidecar is a separate binary that opens the same SQLite file and
//! depends on neither `souffle` nor `tauri`, so anything it shares with the
//! app used to be restated on its side. This crate is the shared declaration:
//! the schema version, the row shapes both processes read, the [`Speaker`]
//! lane tag stored in the `segments` table, and the one piece of rendering
//! logic both processes must agree on, the paragraph grouper in
//! [`paragraphs`].

use serde::{Deserialize, Serialize};

pub mod paragraphs;

/// Schema version 16: `meetings.system_audio` holds what the system-audio
/// leg did over the recording, so a mic-only meeting still says why.
///
/// Both the writer (`db::schema` in the app) and the reader (`souffle-mcp`)
/// build against this number. The sidecar refuses to serve a database whose
/// stored version is higher, because the columns it reads may have moved.
pub const SCHEMA_VERSION: i64 = 16;

/// Who produced a segment in a meeting: the microphone is the local user
/// (`Me`), system audio is everyone else (`Them`). `None` = single-stream
/// session (dictation, or a meeting recorded without system-audio capture).
///
/// Wire and DB encoding is the snake_case variant name, "me" or "them".
///
/// The DB column is free `TEXT` and still holds `spk:<id>` labels from the
/// dropped persistent-speaker feature. [`Speaker::parse`] absorbs those into
/// `None` so old meetings keep loading; that tolerance lives there, not in
/// `Deserialize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Speaker {
    Me,
    Them,
}

impl Speaker {
    /// Wire and DB encoding. Must stay in step with `#[serde(rename_all)]`
    /// above; `speaker_wire_encoding_matches_as_str` in the app proves it does.
    pub fn as_str(self) -> &'static str {
        match self {
            Speaker::Me => "me",
            Speaker::Them => "them",
        }
    }

    /// Plain, non-localized label used by every exporter and by the sidecar.
    pub fn display_name(self) -> &'static str {
        match self {
            Speaker::Me => "Me",
            Speaker::Them => "Them",
        }
    }

    pub fn parse(s: &str) -> Option<Speaker> {
        match s {
            "me" => Some(Speaker::Me),
            "them" => Some(Speaker::Them),
            _ => None,
        }
    }
}

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
