use tauri::State;
use tauri::ipc::Channel;

use crate::settings::AppSettings;
use crate::state::AppState;

/// List all saved meetings
#[tauri::command]
pub fn list_meetings(
    state: State<'_, AppState>,
) -> Result<Vec<crate::transcript::MeetingListItem>, String> {
    state.db.list_meetings()
}

/// Get a full meeting transcript by ID
#[tauri::command]
pub fn get_meeting(
    state: State<'_, AppState>,
    id: String,
) -> Result<crate::transcript::MeetingTranscript, String> {
    state.db.load_meeting(&id)
}

/// Delete a meeting by ID
#[tauri::command]
pub fn delete_meeting(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.db.delete_meeting(&id)
}

/// Check if Ollama is available and list models
#[tauri::command]
pub async fn check_ollama(state: State<'_, AppState>) -> Result<crate::ollama::OllamaStatus, String> {
    let settings = AppSettings::load(&state.db)?;
    Ok(crate::ollama::check_available(Some(&settings.ollama_url)).await)
}

/// Summarize a meeting transcript using Ollama, streaming results back
#[tauri::command]
pub async fn summarize_meeting(
    state: State<'_, AppState>,
    id: String,
    model: String,
    channel: Channel<crate::ollama::SummarizeProgress>,
) -> Result<(), String> {
    let transcript = state.db.load_meeting(&id)?;
    let settings = AppSettings::load(&state.db)?;

    let text: String = transcript
        .segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    if text.is_empty() {
        return Err("Transcript has no text".into());
    }

    let channel_clone = channel.clone();
    let db = state.db.clone();
    let summary = crate::ollama::summarize_stream(&text, &model, Some(&settings.ollama_url), move |progress| {
        let _ = channel_clone.send(progress);
    })
    .await?;

    db.update_meeting_summary(&id, &summary, &model)?;

    Ok(())
}
