use std::path::PathBuf;

use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::db::search::SearchResult;
use crate::engine::TranscriptionSegment;
use crate::export::{self, ExportFormat};
use crate::settings::AppSettings;
use crate::state::AppState;

/// Native save panel, parented to `main` after bringing the app forward.
///
/// The JS dialog plugin parents to whichever webview invoked it. The pill
/// overlay is a non-activating `NSPanel`, and WKWebView swallows OS surfaces
/// the same way it swallows `target="_blank"` (see `open_release_page`):
/// click, nothing opens. Talk to AppKit from this side of the webview.
pub(crate) fn pick_save_path(
    app: &AppHandle,
    file_name: &str,
    extension: &str,
) -> Result<Option<PathBuf>, String> {
    bring_app_forward(app);

    let Some(main) = app.get_webview_window("main") else {
        return Err("Main window is gone; cannot show the save dialog".into());
    };

    let picked = app
        .dialog()
        .file()
        .set_file_name(file_name)
        .add_filter(extension.to_uppercase(), &[extension])
        .set_parent(&main)
        .blocking_save_file();

    match picked {
        Some(path) => path
            .into_path()
            .map(Some)
            .map_err(|e| format!("Save path: {e}")),
        None => Ok(None),
    }
}

fn bring_app_forward(app: &AppHandle) {
    let (tx, rx) = std::sync::mpsc::sync_channel(0);
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        #[cfg(target_os = "macos")]
        crate::tray::activate_app();
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.unminimize();
            let _ = main.show();
            let _ = main.set_focus();
        }
        let _ = tx.send(());
    });
    let _ = rx.recv();
}

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
    Ok(crate::audio::recorder::list_session_files(&meeting_id)
        .into_iter()
        .map(
            |(session_index, path)| crate::transcript::MeetingAudioSession {
                session_index,
                path: path.to_string_lossy().to_string(),
                duration_seconds: None,
            },
        )
        .collect())
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
    segment_indices: Vec<u32>,
    new_text: String,
) -> Result<(), String> {
    use crate::filter::session_terms::derive_corrections_from_edit;
    use crate::lock_ext::MutexExt;

    if segment_indices.is_empty() {
        return Err("Paragraph has no segments".into());
    }

    let new_text = new_text.trim().to_string();
    if new_text.is_empty() {
        return Err("Paragraph text cannot be empty".into());
    }

    let (previous_texts, db_updates, corrections) = {
        let mut acc = state.meeting_accumulator.acquire()?;
        let Some(meeting) = acc.as_mut() else {
            return Err("No meeting is recording".into());
        };
        if meeting.id != meeting_id {
            return Err("Meeting id mismatch".into());
        }

        let indices = unique_ordered_indices(&segment_indices, meeting.new_segments.len())?;
        let speaker = meeting.new_segments[indices[0]].speaker;
        if indices
            .iter()
            .any(|&index| meeting.new_segments[index].speaker != speaker)
        {
            return Err("Paragraph edit spans more than one speaker".into());
        }

        let original_text = indices
            .iter()
            .map(|&index| meeting.new_segments[index].text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        // Bail before touching anything: redistribution would still reshuffle
        // the words across the segments, and with no database write behind it
        // the accumulator would drift from the rows already on disk.
        if original_text == new_text {
            return Ok(());
        }
        let corrections = derive_corrections_from_edit(&original_text, &new_text);

        let previous_texts: Vec<(usize, String)> = indices
            .iter()
            .map(|&index| (index, meeting.new_segments[index].text.clone()))
            .collect();

        redistribute_segment_texts_at(&mut meeting.new_segments, &indices, &new_text);

        let global_base = meeting.existing_segments.len();
        let db_updates: Vec<(i64, String)> = indices
            .iter()
            .copied()
            .filter(|index| *index < meeting.persisted_new_count)
            .map(|index| {
                (
                    (global_base + index) as i64,
                    meeting.new_segments[index].text.clone(),
                )
            })
            .collect();

        (previous_texts, db_updates, corrections)
    };

    // The accumulator drives the summary and the rows still to be flushed, so
    // it must not keep an edit the transcript on disk rejected. Put the old
    // words back before reporting the failure the UI rolls its own copy back
    // on. The mutation stays inside the lock so a flush racing us persists the
    // edited text rather than the text it is about to replace.
    if let Err(e) = state.db.update_segment_texts(&meeting_id, &db_updates) {
        restore_segment_texts(&state, &meeting_id, &previous_texts);
        return Err(e);
    }

    // The text is committed on both sides by now. A dead engine actor only
    // costs the session rewrite rule, so do not report a failure the caller
    // would undo a landed edit for.
    for correction in corrections {
        if let Err(e) = state.engine_actor.add_session_correction(correction) {
            tracing::warn!(error = %e, "Live paragraph edit could not register its session correction");
            break;
        }
    }

    Ok(())
}

/// Undo a live edit in the accumulator after the database refused it.
fn restore_segment_texts(state: &AppState, meeting_id: &str, previous: &[(usize, String)]) {
    use crate::lock_ext::MutexExt;

    let Ok(mut acc) = state.meeting_accumulator.acquire() else {
        return;
    };
    let Some(meeting) = acc.as_mut() else {
        return;
    };
    if meeting.id != meeting_id {
        return;
    }
    for (index, text) in previous {
        if let Some(segment) = meeting.new_segments.get_mut(*index) {
            segment.text.clone_from(text);
        }
    }
}

/// Register a misspelling-to-term pair for the active recording session so
/// later STT output of the same form is rewritten immediately.
#[tauri::command]
#[specta::specta]
pub fn add_session_correction(
    state: State<'_, AppState>,
    misspelling: String,
    term: String,
) -> Result<(), String> {
    use crate::filter::session_terms::SessionCorrection;
    use crate::lock_ext::MutexExt;

    let misspelling = misspelling.trim().to_string();
    let term = term.trim().to_string();
    if misspelling.is_empty() || term.is_empty() {
        return Err("Misspelling and term cannot be empty".into());
    }
    if misspelling == term {
        return Ok(());
    }

    {
        let acc = state.meeting_accumulator.acquire()?;
        if acc.is_none() {
            return Err("No meeting is recording".into());
        }
    }

    state
        .engine_actor
        .add_session_correction(SessionCorrection { misspelling, term })
}

fn unique_ordered_indices(raw: &[u32], len: usize) -> Result<Vec<usize>, String> {
    let mut seen = std::collections::HashSet::new();
    let mut indices = Vec::with_capacity(raw.len());
    for &raw_index in raw {
        let index = raw_index as usize;
        if index >= len {
            return Err("Segment range out of bounds".into());
        }
        if seen.insert(index) {
            indices.push(index);
        }
    }
    if indices.is_empty() {
        return Err("Paragraph has no segments".into());
    }
    Ok(indices)
}

fn redistribute_segment_texts_at(
    segments: &mut [TranscriptionSegment],
    indices: &[usize],
    new_text: &str,
) {
    let words: Vec<&str> = new_text.split_whitespace().collect();
    if indices.is_empty() {
        return;
    }
    if words.is_empty() {
        for &index in indices {
            segments[index].text.clear();
        }
        return;
    }
    if indices.len() == 1 {
        segments[indices[0]].text = new_text.to_string();
        return;
    }
    let last = indices.len() - 1;
    for (offset, &index) in indices.iter().enumerate() {
        if offset < last {
            segments[index].text = words.get(offset).copied().unwrap_or("").to_string();
        } else {
            segments[index].text = words.get(offset..).unwrap_or(&[""]).join(" ");
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

/// Show a native save dialog and write the meeting export. Runs off the
/// webview (see [`pick_save_path`]); cancel is a no-op, not an error.
#[tauri::command]
#[specta::specta]
pub async fn save_meeting_export(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    format: ExportFormat,
) -> Result<(), String> {
    let meeting = state.db.load_meeting(&id)?;
    let filename = export::export_default_filename(&meeting, format);
    let extension = export::export_extension(format).to_string();
    let rendered = export::render_meeting(&meeting, format)?;

    let picked =
        tauri::async_runtime::spawn_blocking(move || pick_save_path(&app, &filename, &extension))
            .await
            .map_err(|e| format!("Save dialog task failed: {e}"))??;

    let Some(path) = picked else {
        return Ok(());
    };
    std::fs::write(&path, rendered).map_err(|e| format!("Write export file: {e}"))
}

/// Suggested filename for a meeting audio export (e.g. `2026-07-09-weekly-sync.ogg`).
#[tauri::command]
#[specta::specta]
pub fn export_meeting_audio_filename(
    state: State<'_, AppState>,
    id: String,
) -> Result<String, String> {
    let meeting = state.db.load_meeting(&id)?;
    Ok(export::export_audio_filename(&meeting))
}

/// Copy recorded audio for a meeting to `path`. One session writes that
/// file; several sessions write `{stem}-1.ogg`, `{stem}-2.ogg`, … next to it.
#[tauri::command]
#[specta::specta]
pub fn export_meeting_audio_to_file(id: String, path: String) -> Result<(), String> {
    let sources: Vec<_> = crate::audio::recorder::list_session_files(&id)
        .into_iter()
        .map(|(_, path)| path)
        .collect();
    export::copy_audio_sessions(&sources, std::path::Path::new(&path))?;
    Ok(())
}

/// Show a native save dialog and copy recorded audio. Same webview
/// parenting issue as [`save_meeting_export`]; cancel is a no-op.
#[tauri::command]
#[specta::specta]
pub async fn save_meeting_audio_export(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let meeting = state.db.load_meeting(&id)?;
    let filename = export::export_audio_filename(&meeting);
    let sources: Vec<_> = crate::audio::recorder::list_session_files(&id)
        .into_iter()
        .map(|(_, path)| path)
        .collect();
    if sources.is_empty() {
        return Err("No recorded audio for this meeting".into());
    }

    let picked =
        tauri::async_runtime::spawn_blocking(move || pick_save_path(&app, &filename, "ogg"))
            .await
            .map_err(|e| format!("Save dialog task failed: {e}"))??;

    let Some(path) = picked else {
        return Ok(());
    };
    export::copy_audio_sessions(&sources, &path)?;
    Ok(())
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

/// Pull the recommended Ollama chat model (`qwen2.5:7b`) into the configured
/// server. Progress is streamed back via the Channel API.
#[tauri::command]
#[specta::specta]
pub async fn pull_recommended_ollama_model(
    state: State<'_, AppState>,
    channel: Channel<crate::summary::OllamaPullProgress>,
) -> Result<String, String> {
    let settings = AppSettings::load(&state.db)?;
    let url = if settings.ollama_url.trim().is_empty() {
        crate::constants::OLLAMA_DEFAULT_URL
    } else {
        settings.ollama_url.trim()
    };
    let model = crate::summary::RECOMMENDED_OLLAMA_MODEL;
    crate::summary::pull_model(url, model, |progress| {
        let _ = channel.send(progress);
    })
    .await?;
    Ok(model.to_string())
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

    let (text, turn_units) = match transcript.edited_transcript {
        Some(ref edited) if !edited.is_empty() => (edited.clone(), None),
        _ => {
            let turns = crate::summary::turns_from_segments(&transcript.segments);
            let text = turns.join("\n");
            (text, Some(turns))
        }
    };

    if text.is_empty() {
        return Err("Transcript has no text".into());
    }

    let final_system_prompt =
        crate::summary::resolve_summary_template_prompt(&settings, template_id.as_deref());
    let output_language = crate::summary::resolve_summary_language(
        settings.meeting_transcription_language,
        &transcript.segments,
        &settings.locale,
    );

    let channel_clone = channel.clone();
    let db = state.db.clone();
    let summary = crate::summary::summarize_stream(
        &text,
        turn_units.as_deref(),
        transcript.notes.as_deref(),
        &transcript.participants,
        &model,
        Some(&settings.ollama_url),
        &final_system_prompt,
        output_language,
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

#[cfg(test)]
mod live_edit {
    use super::*;
    use crate::engine::{Speaker, TranscriptionSegment};

    fn seg(text: &str, speaker: Option<Speaker>) -> TranscriptionSegment {
        TranscriptionSegment {
            text: text.to_string(),
            start_time: 0.0,
            end_time: 0.5,
            is_final: true,
            language: None,
            confidence: None,
            speaker,
        }
    }

    #[test]
    fn redistribute_at_indices_leaves_the_other_speaker() {
        let mut segments = vec![
            seg("hello", Some(Speaker::Me)),
            seg("hi", Some(Speaker::Them)),
            seg("how", Some(Speaker::Me)),
            seg("good", Some(Speaker::Them)),
            seg("are", Some(Speaker::Me)),
            seg("thanks", Some(Speaker::Them)),
            seg("you", Some(Speaker::Me)),
        ];
        redistribute_segment_texts_at(&mut segments, &[0, 2, 4, 6], "hello how are we");
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.text.as_str())
                .collect::<Vec<_>>(),
            ["hello", "hi", "how", "good", "are", "thanks", "we"]
        );
    }

    #[test]
    fn unique_ordered_indices_rejects_out_of_bounds() {
        assert!(unique_ordered_indices(&[0, 9], 3).is_err());
        assert_eq!(
            unique_ordered_indices(&[0, 2, 0, 4], 7).unwrap(),
            vec![0, 2, 4]
        );
    }

    #[test]
    fn original_text_joins_listed_segments_in_display_order() {
        let segments = vec![
            seg("hello", Some(Speaker::Me)),
            seg("hi", Some(Speaker::Them)),
            seg("how", Some(Speaker::Me)),
        ];
        let indices = unique_ordered_indices(&[0, 2], segments.len()).unwrap();
        let original: String = indices
            .iter()
            .map(|&index| segments[index].text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(original, "hello how");
        assert_eq!(segments[1].text, "hi");
    }
}
