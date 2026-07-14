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
    )?;

    state.db.load_meeting(&request.meeting_id)
}
