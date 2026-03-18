use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::engine::TranscriptionSegment;

/// Full meeting transcript stored as JSON
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingTranscript {
    pub id: String,
    pub title: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_seconds: f64,
    pub engine: String,
    pub segments: Vec<TranscriptionSegment>,
    pub summary: Option<String>,
    pub summary_model: Option<String>,
    pub summary_generated_at: Option<DateTime<Utc>>,
}

/// Lightweight item for listing meetings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingListItem {
    pub id: String,
    pub title: String,
    pub started_at: DateTime<Utc>,
    pub duration_seconds: f64,
    pub has_summary: bool,
}

impl From<&MeetingTranscript> for MeetingListItem {
    fn from(t: &MeetingTranscript) -> Self {
        Self {
            id: t.id.clone(),
            title: t.title.clone(),
            started_at: t.started_at,
            duration_seconds: t.duration_seconds,
            has_summary: t.summary.is_some(),
        }
    }
}

fn meetings_dir() -> PathBuf {
    dirs_next::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.souffle.app")
        .join("meetings")
}

pub fn save_meeting(transcript: &MeetingTranscript) -> Result<(), String> {
    let dir = meetings_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Create meetings dir: {e}"))?;
    let path = dir.join(format!("{}.json", transcript.id));
    let json = serde_json::to_string_pretty(transcript).map_err(|e| format!("Serialize: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("Write: {e}"))?;
    Ok(())
}

pub fn load_meeting(id: &str) -> Result<MeetingTranscript, String> {
    let path = meetings_dir().join(format!("{id}.json"));
    if !path.exists() {
        return Err(format!("Meeting not found: {id}"));
    }
    let json = std::fs::read_to_string(&path).map_err(|e| format!("Read: {e}"))?;
    serde_json::from_str(&json).map_err(|e| format!("Deserialize: {e}"))
}

pub fn list_meetings() -> Result<Vec<MeetingListItem>, String> {
    let dir = meetings_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut items = Vec::new();
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("Read dir: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("Entry: {e}"))?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let json = std::fs::read_to_string(&path).map_err(|e| format!("Read: {e}"))?;
            if let Ok(transcript) = serde_json::from_str::<MeetingTranscript>(&json) {
                items.push(MeetingListItem::from(&transcript));
            }
        }
    }
    // Sort by date, newest first
    items.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Ok(items)
}

pub fn delete_meeting(id: &str) -> Result<(), String> {
    let path = meetings_dir().join(format!("{id}.json"));
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("Delete: {e}"))?;
    }
    Ok(())
}

pub fn update_meeting_summary(
    id: &str,
    summary: &str,
    model: &str,
) -> Result<(), String> {
    let mut transcript = load_meeting(id)?;
    transcript.summary = Some(summary.to_string());
    transcript.summary_model = Some(model.to_string());
    transcript.summary_generated_at = Some(Utc::now());
    save_meeting(&transcript)
}
