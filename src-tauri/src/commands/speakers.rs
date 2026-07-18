use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::AppState;
use crate::transcript::{MeetingSpeaker, MeetingTranscript};

/// Rename a persistent speaker. The new name appears everywhere that speaker
/// id is referenced because segment labels stay `spk:<id>`.
#[tauri::command]
#[specta::specta]
pub fn rename_speaker(state: State<'_, AppState>, id: i64, name: String) -> Result<(), String> {
    state.db.rename_speaker(id, &name)
}

/// All persistent speakers in the database, for pickers when retagging.
#[tauri::command]
#[specta::specta]
pub fn list_speakers(state: State<'_, AppState>) -> Result<Vec<MeetingSpeaker>, String> {
    Ok(state
        .db
        .list_speakers()?
        .into_iter()
        .map(|record| MeetingSpeaker {
            id: record.id,
            name: record.name,
        })
        .collect())
}

/// One remembered voice with usage counts, for the Settings management list.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct SpeakerProfile {
    pub id: i64,
    pub name: String,
    /// RFC3339 timestamp of the last meeting this voice was recognized in.
    pub last_seen_at: String,
    pub meeting_count: u32,
    pub segment_count: u32,
    pub is_me: bool,
}

/// All persistent speakers with usage counts, most recently seen first.
#[tauri::command]
#[specta::specta]
pub fn list_speaker_profiles(state: State<'_, AppState>) -> Result<Vec<SpeakerProfile>, String> {
    let usage = state.db.speaker_usage()?;
    let mut profiles: Vec<SpeakerProfile> = state
        .db
        .list_speakers()?
        .into_iter()
        .map(|record| {
            let counts = usage.get(&record.id).copied().unwrap_or_default();
            SpeakerProfile {
                id: record.id,
                name: record.name,
                last_seen_at: record.last_seen_at.to_rfc3339(),
                meeting_count: counts.meeting_count.max(0) as u32,
                segment_count: counts.segment_count.max(0) as u32,
                is_me: record.is_me,
            }
        })
        .collect();
    profiles.sort_by(|a, b| b.last_seen_at.cmp(&a.last_seen_at));
    Ok(profiles)
}

/// Delete a persistent speaker; every segment that referenced it goes back
/// to unlabeled, and future meetings can no longer match this voice.
#[tauri::command]
#[specta::specta]
pub fn delete_speaker(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state.db.delete_speaker(id)
}

/// Merge one persistent speaker into another: segments and voice embeddings
/// move to the target, and the source is deleted.
#[tauri::command]
#[specta::specta]
pub fn merge_speakers(state: State<'_, AppState>, source_id: i64, target_id: i64) -> Result<(), String> {
    state.db.merge_speakers(source_id, target_id)
}

/// Flag (or unflag) a persistent speaker as the app's user.
#[tauri::command]
#[specta::specta]
pub fn set_speaker_is_me(state: State<'_, AppState>, id: i64, is_me: bool) -> Result<(), String> {
    state.db.set_speaker_is_me(id, is_me)
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct RetagMeetingSpeakerRequest {
    pub meeting_id: String,
    /// Persistent speaker id currently on the segments being retagged.
    pub from_speaker_id: i64,
    /// Retag only these segment indices (`sort_order`). Omit or pass an empty
    /// vec to retag every `spk:from_speaker_id` segment in this meeting.
    #[serde(default)]
    pub sort_orders: Vec<u64>,
    /// Assign to an existing persistent speaker.
    pub to_speaker_id: Option<i64>,
    /// Create a new persistent speaker with this name and assign to it.
    pub new_speaker_name: Option<String>,
    /// When true, also move this meeting's voice embeddings for the source
    /// speaker to the target, so future matching benefits from the
    /// correction. When false, the retag only relabels this meeting.
    #[serde(default)]
    pub remember: bool,
}

/// Reassign persistent-speaker labels within one meeting. Me/Them segments
/// are never touched. Returns the reloaded meeting so the UI can refresh.
#[tauri::command]
#[specta::specta]
pub fn retag_meeting_speaker(
    state: State<'_, AppState>,
    request: RetagMeetingSpeakerRequest,
) -> Result<MeetingTranscript, String> {
    let scope = if request.sort_orders.is_empty() {
        None
    } else {
        Some(request.sort_orders.as_slice())
    };

    state.db.retag_meeting_speaker_labels(
        &request.meeting_id,
        request.from_speaker_id,
        request.to_speaker_id,
        request.new_speaker_name.as_deref(),
        scope,
        request.remember,
    )?;

    state.db.load_meeting(&request.meeting_id)
}
