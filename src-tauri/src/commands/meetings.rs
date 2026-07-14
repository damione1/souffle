use tauri::State;
use tauri::ipc::Channel;

use crate::db::search::SearchResult;
use crate::engine::TranscriptionSegment;
use crate::export::{self, ExportFormat};
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

/// Delete a meeting by ID, including any recorded audio.
#[tauri::command]
#[specta::specta]
pub fn delete_meeting(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.db.delete_meeting(&id)?;

    let recordings_dir = crate::audio::recorder::meeting_recordings_dir(&id);
    if recordings_dir.exists()
        && let Err(e) = std::fs::remove_dir_all(&recordings_dir)
    {
        tracing::warn!(meeting_id = %id, "Failed to delete meeting recordings: {e}");
    }

    Ok(())
}

/// List the recorded audio files for a meeting (empty if recording was never
/// enabled, or none survived retention). Reads the filesystem directly —
/// nothing here is persisted in the database.
#[tauri::command]
#[specta::specta]
pub fn get_meeting_audio(
    meeting_id: String,
) -> Result<Vec<crate::transcript::MeetingAudioSession>, String> {
    let dir = crate::audio::recorder::meeting_recordings_dir(&meeting_id);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };

    let mut sessions: Vec<crate::transcript::MeetingAudioSession> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("ogg") {
                return None;
            }
            let session_index = path.file_stem()?.to_str()?.parse::<usize>().ok()?;
            Some(crate::transcript::MeetingAudioSession {
                session_index,
                path: path.to_string_lossy().to_string(),
                duration_seconds: None,
            })
        })
        .collect();

    sessions.sort_by_key(|session| session.session_index);
    Ok(sessions)
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

/// Apply a live paragraph edit during an active meeting: patch the edited
/// segments in the accumulator (and on disk when already flushed), and
/// register session corrections so later STT output of the same misspelling
/// is rewritten for the rest of this recording session.
#[tauri::command]
#[specta::specta]
pub fn apply_live_paragraph_edit(
    state: State<'_, AppState>,
    meeting_id: String,
    segment_start: u32,
    segment_end: u32,
    new_text: String,
) -> Result<(), String> {
    use crate::filter::session_terms::derive_corrections_from_edit;
    use crate::lock_ext::MutexExt;

    let segment_start = segment_start as usize;
    let segment_end = segment_end as usize;
    if segment_end <= segment_start {
        return Err("Invalid segment range".into());
    }

    let new_text = new_text.trim().to_string();
    if new_text.is_empty() {
        return Err("Paragraph text cannot be empty".into());
    }

    let (original_text, db_updates, corrections) = {
        let mut acc = state.meeting_accumulator.acquire()?;
        let Some(meeting) = acc.as_mut() else {
            return Err("No meeting is recording".into());
        };
        if meeting.id != meeting_id {
            return Err("Meeting id mismatch".into());
        }
        if segment_end > meeting.new_segments.len() {
            return Err("Segment range out of bounds".into());
        }

        let slice = &meeting.new_segments[segment_start..segment_end];
        let original_text = slice
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let corrections = derive_corrections_from_edit(&original_text, &new_text);

        redistribute_segment_texts(
            &mut meeting.new_segments[segment_start..segment_end],
            &new_text,
        );

        let global_base = meeting.existing_segments.len();
        let db_updates: Vec<(i64, String)> = (segment_start..segment_end)
            .filter(|index| *index < meeting.persisted_new_count)
            .map(|index| {
                (
                    (global_base + index) as i64,
                    meeting.new_segments[index].text.clone(),
                )
            })
            .collect();

        (original_text, db_updates, corrections)
    };

    if original_text == new_text {
        return Ok(());
    }

    if !db_updates.is_empty() {
        state.db.update_segment_texts(&meeting_id, &db_updates)?;
    }

    for correction in corrections {
        state.engine_actor.add_session_correction(correction)?;
    }

    Ok(())
}

fn redistribute_segment_texts(segments: &mut [TranscriptionSegment], new_text: &str) {
    let words: Vec<&str> = new_text.split_whitespace().collect();
    if segments.is_empty() {
        return;
    }
    if words.is_empty() {
        for segment in segments.iter_mut() {
            segment.text.clear();
        }
        return;
    }
    if segments.len() == 1 {
        segments[0].text = new_text.to_string();
        return;
    }
    let last_index = segments.len() - 1;
    for (index, segment) in segments.iter_mut().enumerate() {
        if index < last_index {
            segment.text = words.get(index).copied().unwrap_or("").to_string();
        } else {
            segment.text = words.get(index..).unwrap_or(&[""]).join(" ");
        }
    }
}

/// Render a meeting export without writing to disk. Used by tests and, if
/// ever needed, a clipboard-copy affordance.
#[tauri::command]
#[specta::specta]
pub fn export_meeting_preview(
    state: State<'_, AppState>,
    id: String,
    format: ExportFormat,
) -> Result<String, String> {
    let meeting = state.db.load_meeting(&id)?;
    export::render_meeting(&meeting, format)
}

/// Suggested filename for a meeting export (e.g. `2026-07-09-weekly-sync.md`),
/// used as the save dialog's default path.
#[tauri::command]
#[specta::specta]
pub fn export_meeting_filename(
    state: State<'_, AppState>,
    id: String,
    format: ExportFormat,
) -> Result<String, String> {
    let meeting = state.db.load_meeting(&id)?;
    Ok(export::export_default_filename(&meeting, format))
}

/// Render a meeting export and write it to `path`. The save dialog itself
/// (picking `path`) runs frontend-side via the dialog plugin.
#[tauri::command]
#[specta::specta]
pub fn export_meeting_to_file(
    state: State<'_, AppState>,
    id: String,
    format: ExportFormat,
    path: String,
) -> Result<(), String> {
    let meeting = state.db.load_meeting(&id)?;
    let rendered = export::render_meeting(&meeting, format)?;
    std::fs::write(&path, rendered).map_err(|e| format!("Write export file: {e}"))
}

/// List available summary providers and models (Ollama + Apple Intelligence).
#[tauri::command]
#[specta::specta]
pub async fn check_summary_providers(
    state: State<'_, AppState>,
) -> Result<crate::summary::SummaryProvidersStatus, String> {
    let settings = AppSettings::load(&state.db)?;
    Ok(crate::summary::check_providers(&settings.ollama_url).await)
}

/// Summarize a meeting transcript using the selected provider, streaming results back.
///
/// `template_id` picks the summary template controlling the final-pass system
/// prompt; `None` (or an unknown id) falls back to the default template
/// configured in settings, so automatic summarization always uses the default.
#[tauri::command]
#[specta::specta]
pub async fn summarize_meeting(
    state: State<'_, AppState>,
    id: String,
    model: String,
    template_id: Option<String>,
    channel: Channel<crate::summary::SummarizeProgress>,
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

    let final_system_prompt =
        crate::summary::resolve_summary_template_prompt(&settings, template_id.as_deref());

    let channel_clone = channel.clone();
    let db = state.db.clone();
    let summary = crate::summary::summarize_stream(
        &text,
        transcript.notes.as_deref(),
        &transcript.participants,
        &model,
        Some(&settings.ollama_url),
        &final_system_prompt,
        move |progress| {
            let _ = channel_clone.send(progress);
        },
    )
    .await?;

    let _ = channel.send(crate::summary::SummarizeProgress {
        text: String::new(),
        done: false,
        stage: crate::summary::SummarizeStage::Extract,
        current: None,
        total: None,
    });

    let structured_result = crate::summary::extract_structured_summary(
        &summary,
        transcript.notes.as_deref(),
        &transcript.participants,
        &model,
        Some(&settings.ollama_url),
    )
    .await;

    let (structured, extract_warning) =
        crate::summary::structured_extract_for_persist(structured_result);
    if let Some(warning) = extract_warning {
        tracing::warn!("Structured summary extract failed, saving prose only: {warning}");
    }

    db.update_meeting_summary(&id, &summary, structured.as_ref(), &model)?;

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
