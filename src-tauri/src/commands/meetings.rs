use tauri::State;
use tauri::ipc::Channel;

use crate::db::search::SearchResult;
use crate::settings::AppSettings;
use crate::state::AppState;

/// List all saved meetings
#[tauri::command]
#[specta::specta]
pub fn list_meetings(
    state: State<'_, AppState>,
) -> Result<Vec<crate::transcript::MeetingListItem>, String> {
    state.db.list_meetings()
}

/// Get a full meeting transcript by ID
#[tauri::command]
#[specta::specta]
pub fn get_meeting(
    state: State<'_, AppState>,
    id: String,
) -> Result<crate::transcript::MeetingTranscript, String> {
    state.db.load_meeting(&id)
}

/// Delete a meeting by ID
#[tauri::command]
#[specta::specta]
pub fn delete_meeting(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.db.delete_meeting(&id)
}

/// Save the user's live meeting notes. Targets the in-memory accumulator
/// while that meeting is still recording (it only reaches the DB at stop),
/// the DB otherwise.
#[tauri::command]
#[specta::specta]
pub fn save_meeting_notes(
    state: State<'_, AppState>,
    id: String,
    notes: Option<String>,
) -> Result<(), String> {
    let notes = notes
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());

    {
        use crate::lock_ext::MutexExt;
        let mut acc = state.meeting_accumulator.acquire()?;
        if let Some(ref mut meeting) = *acc
            && meeting.id == id
        {
            meeting.notes = notes;
            return Ok(());
        }
    }

    state.db.save_meeting_notes(&id, notes.as_deref())
}

/// Rename a meeting. Targets the in-memory accumulator while that meeting
/// is still recording (it only reaches the DB at stop), the DB otherwise.
#[tauri::command]
#[specta::specta]
pub fn rename_meeting(state: State<'_, AppState>, id: String, title: String) -> Result<(), String> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("Title cannot be empty".into());
    }

    {
        use crate::lock_ext::MutexExt;
        let mut acc = state.meeting_accumulator.acquire()?;
        if let Some(ref mut meeting) = *acc
            && meeting.id == id
        {
            meeting.title = title;
            return Ok(());
        }
    }

    state.db.update_meeting_title(&id, &title)
}

/// Save an edited transcript for a meeting
#[tauri::command]
#[specta::specta]
pub fn save_edited_transcript(
    state: State<'_, AppState>,
    id: String,
    edited_transcript: Option<String>,
) -> Result<(), String> {
    state
        .db
        .save_edited_transcript(&id, edited_transcript.as_deref())
}

/// Check if Ollama is available and list models
#[tauri::command]
#[specta::specta]
pub async fn check_ollama(
    state: State<'_, AppState>,
) -> Result<crate::ollama::OllamaStatus, String> {
    let settings = AppSettings::load(&state.db)?;
    Ok(crate::ollama::check_available(Some(&settings.ollama_url)).await)
}

/// Summarize a meeting transcript using Ollama, streaming results back
#[tauri::command]
#[specta::specta]
pub async fn summarize_meeting(
    state: State<'_, AppState>,
    id: String,
    model: String,
    channel: Channel<crate::ollama::SummarizeProgress>,
) -> Result<(), String> {
    let transcript = state.db.load_meeting(&id)?;
    let settings = AppSettings::load(&state.db)?;

    let text = match transcript.edited_transcript {
        Some(ref edited) if !edited.is_empty() => edited.clone(),
        _ => transcript
            .segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    };

    if text.is_empty() {
        return Err("Transcript has no text".into());
    }

    let channel_clone = channel.clone();
    let db = state.db.clone();
    let summary = crate::ollama::summarize_stream(
        &text,
        transcript.notes.as_deref(),
        &transcript.participants,
        &model,
        Some(&settings.ollama_url),
        move |progress| {
            let _ = channel_clone.send(progress);
        },
    )
    .await?;

    db.update_meeting_summary(&id, &summary, &model)?;

    Ok(())
}

/// Full-text search across meetings and dictation entries
#[tauri::command]
#[specta::specta]
pub fn search_text(
    state: State<'_, AppState>,
    query: String,
    limit: Option<i64>,
) -> Result<Vec<SearchResult>, String> {
    if query.trim().is_empty() {
        return Ok(vec![]);
    }
    state.db.search_text(&query, limit.unwrap_or(20))
}
