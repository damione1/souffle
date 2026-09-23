//! Renders a `MeetingTranscript` into Markdown, JSON, SRT, or VTT text for
//! the single-meeting export feature. Pure string rendering: no I/O here,
//! `commands::meetings` handles the file dialog / filesystem write.
//!
//! Also hosts the small filesystem-naming helpers (`archive_folder_name`,
//! `unique_dir`) shared by the full-archive export in `crate::archive`, since
//! they follow the same date+slug naming convention as [`export_default_filename`].

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use souffle_schema::paragraphs;

use crate::engine::{Speaker, TranscriptionSegment};
use crate::transcript::{MeetingTranscript, StructuredSummary};

/// Minimum on-screen duration given to a subtitle cue whose segment has a
/// zero or inverted end time, so SRT/VTT players never render a cue with
/// zero (or negative) length.
const MIN_CUE_DURATION_SECONDS: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Markdown,
    Json,
    Srt,
    Vtt,
}

/// File extension (without the dot) for a given export format.
pub fn export_extension(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Markdown => "md",
        ExportFormat::Json => "json",
        ExportFormat::Srt => "srt",
        ExportFormat::Vtt => "vtt",
    }
}

/// Lowercase, alphanumeric-only slug: everything else collapses to a single
/// hyphen, and leading/trailing hyphens are trimmed. Falls back to
/// `"meeting"` when nothing alphanumeric survives (empty title, emoji-only
/// title, etc). `pub(crate)` so `crate::archive` can build the same
/// `date-slug` folder names it uses for per-meeting export filenames.
pub(crate) fn slugify(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut last_was_separator = false;
    for ch in input.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            result.push(ch);
            last_was_separator = false;
        } else if !last_was_separator {
            result.push('-');
            last_was_separator = true;
        }
    }
    let trimmed = result.trim_matches('-');
    if trimmed.is_empty() {
        "meeting".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Suggested filename for a meeting export, e.g. `2026-07-09-weekly-sync.md`.
pub fn export_default_filename(meeting: &MeetingTranscript, format: ExportFormat) -> String {
    let date = meeting.started_at.format("%Y-%m-%d");
    let slug = slugify(&meeting.title);
    format!("{date}-{slug}.{}", export_extension(format))
}

/// Suggested filename for a meeting audio export, e.g. `2026-07-09-weekly-sync.ogg`.
pub fn export_audio_filename(meeting: &MeetingTranscript) -> String {
    let date = meeting.started_at.format("%Y-%m-%d");
    let slug = slugify(&meeting.title);
    format!("{date}-{slug}.ogg")
}

/// Base folder name for a full data archive, e.g. `souffle-export-2026-07-09`.
/// Callers that need a name guaranteed not to collide with an existing
/// directory should pass this into [`unique_dir`].
pub fn archive_folder_name(now: DateTime<Utc>) -> String {
    format!("souffle-export-{}", now.format("%Y-%m-%d"))
}

/// `parent/base`, or `parent/base-2`, `parent/base-3`, ... if that path
/// already exists on disk. Probes the filesystem rather than tracking names
/// in memory, so it also works for disambiguating sibling directories
/// created earlier in the same run (e.g. two meetings sharing a date+title).
pub fn unique_dir(parent: &Path, base: &str) -> PathBuf {
    let candidate = parent.join(base);
    if !candidate.exists() {
        return candidate;
    }
    let mut suffix = 2;
    loop {
        let candidate = parent.join(format!("{base}-{suffix}"));
        if !candidate.exists() {
            return candidate;
        }
        suffix += 1;
    }
}

/// `path`, or `stem-2.ext`, `stem-3.ext`, ... if that path already exists.
/// Same collision probe as [`unique_dir`], for files rather than folders.
pub fn unique_file(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("meeting");
    let ext = path.extension().and_then(|s| s.to_str());
    let mut suffix = 2;
    loop {
        let name = match ext {
            Some(ext) => format!("{stem}-{suffix}.{ext}"),
            None => format!("{stem}-{suffix}"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
        suffix += 1;
    }
}

/// Copy recorded session files to `dest` (the user-chosen `.ogg` path).
///
/// One session writes `dest`. Several sessions write `{stem}-1.ogg`,
/// `{stem}-2.ogg`, … next to it, probing with [`unique_file`] so a leftover
/// file from a previous export isn't overwritten.
pub fn copy_audio_sessions(sources: &[PathBuf], dest: &Path) -> Result<Vec<PathBuf>, String> {
    if sources.is_empty() {
        return Err("No recorded audio for this meeting".into());
    }

    let mut written = Vec::with_capacity(sources.len());
    if sources.len() == 1 {
        std::fs::copy(&sources[0], dest).map_err(|e| format!("Copy recording: {e}"))?;
        written.push(dest.to_path_buf());
        return Ok(written);
    }

    let parent = dest.parent().unwrap_or(Path::new("."));
    let stem = dest
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("meeting");
    for (index, src) in sources.iter().enumerate() {
        let candidate = parent.join(format!("{}-{}.ogg", stem, index + 1));
        let out = unique_file(&candidate);
        std::fs::copy(src, &out).map_err(|e| format!("Copy recording: {e}"))?;
        written.push(out);
    }
    Ok(written)
}

/// Render a meeting into the requested export format.
pub fn render_meeting(meeting: &MeetingTranscript, format: ExportFormat) -> Result<String, String> {
    match format {
        ExportFormat::Markdown => Ok(render_markdown(meeting)),
        ExportFormat::Json => render_json(meeting),
        ExportFormat::Srt => Ok(render_srt(meeting)),
        ExportFormat::Vtt => Ok(render_vtt(meeting)),
    }
}

fn humanize_duration(total_seconds: f64) -> String {
    let total = total_seconds.max(0.0).round() as i64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;

    let mut parts = Vec::new();
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if hours > 0 || minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    parts.push(format!("{seconds}s"));
    parts.join(" ")
}

fn render_markdown(meeting: &MeetingTranscript) -> String {
    let mut out = String::new();

    out.push_str(&format!("# {}\n\n", meeting.title));

    out.push_str(&format!(
        "- **Date:** {}\n",
        meeting.started_at.format("%Y-%m-%d %H:%M UTC")
    ));
    out.push_str(&format!(
        "- **Duration:** {}\n",
        humanize_duration(meeting.duration_seconds)
    ));
    if !meeting.participants.is_empty() {
        let names = meeting
            .participants
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("- **Participants:** {names}\n"));
    }
    out.push_str(&format!(
        "- **Engine:** {}\n",
        meeting.transcription_profile.engine_label
    ));

    if let Some(notes) = non_empty(meeting.notes.as_deref()) {
        out.push_str("\n## Notes\n\n");
        out.push_str(notes);
        out.push('\n');
    }

    if let Some(summary) = non_empty(meeting.summary.as_deref()) {
        out.push_str("\n## Summary\n\n");
        out.push_str(summary);
        out.push('\n');
    }

    if let Some(structured) = meeting.structured_summary.as_ref() {
        render_structured_markdown(&mut out, structured);
    }

    out.push_str("\n## Transcript\n\n");
    out.push_str(&render_transcript_text(meeting));
    out.push('\n');

    out
}

/// Just the transcript, as Markdown paragraphs with speaker/timestamp
/// prefixes - the edited transcript verbatim when there is one, otherwise
/// the same paragraph grouping `render_markdown`'s Transcript section uses.
/// Factored out of `render_markdown` so the meeting detail's "Copy" action
/// (souffle-slint) can copy just this text to the clipboard without also
/// carrying the notes/summary sections around it.
pub fn render_transcript_text(meeting: &MeetingTranscript) -> String {
    match non_empty(meeting.edited_transcript.as_deref()) {
        Some(edited) => edited.to_string(),
        None => {
            let grouped = paragraphs::group_into_paragraphs(
                &meeting.segments,
                paragraphs::PAUSE_THRESHOLD_SECONDS,
            );
            grouped
                .iter()
                .map(render_paragraph_markdown)
                .collect::<Vec<_>>()
                .join("\n\n")
        }
    }
}

fn non_empty(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|t| !t.is_empty())
}

fn render_structured_markdown(out: &mut String, structured: &StructuredSummary) {
    let has_decisions = !structured.decisions.is_empty();
    let has_actions = !structured.action_items.is_empty();
    let has_questions = !structured.open_questions.is_empty();
    if !has_decisions && !has_actions && !has_questions {
        return;
    }

    out.push_str("\n## Structured Summary\n\n");

    if has_decisions {
        out.push_str("### Decisions\n\n");
        for decision in &structured.decisions {
            out.push_str("- ");
            out.push_str(decision);
            out.push('\n');
        }
        out.push('\n');
    }

    if has_actions {
        out.push_str("### Action Items\n\n");
        for item in &structured.action_items {
            out.push_str("- ");
            if let Some(owner) = non_empty(item.owner.as_deref()) {
                out.push_str("**");
                out.push_str(owner);
                out.push_str("**: ");
            }
            out.push_str(&item.text);
            out.push('\n');
        }
        out.push('\n');
    }

    if has_questions {
        out.push_str("### Open Questions\n\n");
        for question in &structured.open_questions {
            out.push_str("- ");
            out.push_str(question);
            out.push('\n');
        }
        out.push('\n');
    }
}

fn render_paragraph_markdown(p: &paragraphs::Paragraph) -> String {
    match p.speaker {
        Some(speaker) => format!(
            "**{}** [{}] {}",
            speaker.display_name(),
            p.timestamp,
            p.text
        ),
        None => format!("[{}] {}", p.timestamp, p.text),
    }
}

fn render_json(meeting: &MeetingTranscript) -> Result<String, String> {
    serde_json::to_string_pretty(meeting).map_err(|e| format!("Serialize meeting: {e}"))
}

/// Diarized meetings interleave Me (mic) and Them (system audio) segments in
/// storage order per processing frame, not strictly by time, so every export
/// renderer must sort a copy by start time to read as a conversation and
/// keep cue timestamps monotonic. Non-diarized streams keep storage order:
/// legacy window-relative timestamps must not be reordered. Same rule as
/// `souffle_schema::paragraphs::group_into_paragraphs`; the sort is stable,
/// so equal start times preserve storage order too.
fn time_ordered_segments(segments: &[TranscriptionSegment]) -> Vec<&TranscriptionSegment> {
    let diarized = segments.iter().any(|s| s.speaker.is_some());
    let mut ordered: Vec<&TranscriptionSegment> = segments.iter().collect();
    if diarized {
        ordered.sort_by(|a, b| {
            a.start_time
                .partial_cmp(&b.start_time)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    ordered
}

fn speaker_prefix(speaker: Option<Speaker>, text: &str) -> String {
    match speaker {
        Some(speaker) => format!("{}: {text}", speaker.display_name()),
        None => text.to_string(),
    }
}

/// `start_time`/`end_time` clamped to a valid, non-zero-length cue window.
fn cue_window(start_time: f64, end_time: f64) -> (f64, f64) {
    let start = start_time.max(0.0);
    let end = if end_time > start {
        end_time
    } else {
        start + MIN_CUE_DURATION_SECONDS
    };
    (start, end)
}

fn srt_timestamp(seconds: f64) -> String {
    let total_millis = (seconds.max(0.0) * 1000.0).round() as i64;
    let hours = total_millis / 3_600_000;
    let minutes = (total_millis % 3_600_000) / 60_000;
    let secs = (total_millis % 60_000) / 1000;
    let millis = total_millis % 1000;
    format!("{hours:02}:{minutes:02}:{secs:02},{millis:03}")
}

fn vtt_timestamp(seconds: f64) -> String {
    srt_timestamp(seconds).replace(',', ".")
}

fn render_srt(meeting: &MeetingTranscript) -> String {
    let mut out = String::new();
    let mut index = 1u32;
    for seg in time_ordered_segments(&meeting.segments) {
        let text = seg.text.trim();
        if text.is_empty() {
            continue;
        }
        let (start, end) = cue_window(seg.start_time, seg.end_time);
        out.push_str(&format!(
            "{index}\n{} --> {}\n{}\n\n",
            srt_timestamp(start),
            srt_timestamp(end),
            speaker_prefix(seg.speaker, text),
        ));
        index += 1;
    }
    finish_cue_block(out)
}

fn render_vtt(meeting: &MeetingTranscript) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for seg in time_ordered_segments(&meeting.segments) {
        let text = seg.text.trim();
        if text.is_empty() {
            continue;
        }
        let (start, end) = cue_window(seg.start_time, seg.end_time);
        out.push_str(&format!(
            "{} --> {}\n{}\n\n",
            vtt_timestamp(start),
            vtt_timestamp(end),
            speaker_prefix(seg.speaker, text),
        ));
    }
    finish_cue_block(out)
}

/// Trailing blank line between cues is a separator, not part of the file;
/// trim it and end with exactly one newline.
fn finish_cue_block(out: String) -> String {
    format!("{}\n", out.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::fixtures::{sample_meeting, sample_segment};
    use crate::transcript::StructuredActionItem;

    fn diarized_segment(
        text: &str,
        start: f64,
        end: f64,
        speaker: Speaker,
    ) -> crate::engine::TranscriptionSegment {
        let mut seg = sample_segment(text, start, end);
        seg.speaker = Some(speaker);
        seg
    }

    // ── slugify / filename ──────────────────────────────────────────────

    #[test]
    fn slugify_basic_title() {
        let mut meeting = sample_meeting("m1");
        meeting.title = "Weekly Sync".to_string();
        meeting.started_at = "2026-07-09T10:00:00Z".parse().unwrap();
        assert_eq!(
            export_default_filename(&meeting, ExportFormat::Markdown),
            "2026-07-09-weekly-sync.md"
        );
    }

    #[test]
    fn slugify_collapses_punctuation_and_spaces() {
        let mut meeting = sample_meeting("m1");
        meeting.title = "Q3   Planning -- Budget & Roadmap!!".to_string();
        let filename = export_default_filename(&meeting, ExportFormat::Json);
        assert!(filename.ends_with(".json"));
        assert!(!filename.contains("--"));
        assert!(!filename.contains(' '));
    }

    #[test]
    fn slugify_strips_accents_to_ascii_only() {
        let mut meeting = sample_meeting("m1");
        meeting.title = "Café Meeting".to_string();
        let filename = export_default_filename(&meeting, ExportFormat::Srt);
        // Accented chars are not ASCII alphanumeric, so they collapse into
        // the hyphen separator rather than surviving into the slug.
        assert!(filename.contains("caf-meeting") || filename.contains("caf-meeting-"));
        assert!(filename.is_ascii());
    }

    #[test]
    fn slugify_emoji_only_title_falls_back() {
        let mut meeting = sample_meeting("m1");
        meeting.title = "🎉🎉🎉".to_string();
        let filename = export_default_filename(&meeting, ExportFormat::Vtt);
        assert!(filename.ends_with("-meeting.vtt"));
    }

    #[test]
    fn slugify_empty_title_falls_back() {
        let mut meeting = sample_meeting("m1");
        meeting.title = "".to_string();
        let filename = export_default_filename(&meeting, ExportFormat::Markdown);
        assert!(filename.ends_with("-meeting.md"));
    }

    #[test]
    fn export_extension_matches_format() {
        assert_eq!(export_extension(ExportFormat::Markdown), "md");
        assert_eq!(export_extension(ExportFormat::Json), "json");
        assert_eq!(export_extension(ExportFormat::Srt), "srt");
        assert_eq!(export_extension(ExportFormat::Vtt), "vtt");
    }

    // ── archive folder naming ───────────────────────────────────────────

    #[test]
    fn archive_folder_name_formats_date() {
        let now: chrono::DateTime<chrono::Utc> = "2026-07-09T14:32:00Z".parse().unwrap();
        assert_eq!(archive_folder_name(now), "souffle-export-2026-07-09");
    }

    #[test]
    fn unique_dir_returns_base_when_free() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = unique_dir(dir.path(), "souffle-export-2026-07-09");
        assert_eq!(result, dir.path().join("souffle-export-2026-07-09"));
    }

    #[test]
    fn unique_dir_appends_suffix_on_collision() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("souffle-export-2026-07-09")).unwrap();

        let result = unique_dir(dir.path(), "souffle-export-2026-07-09");
        assert_eq!(result, dir.path().join("souffle-export-2026-07-09-2"));
    }

    #[test]
    fn unique_dir_probes_past_multiple_collisions() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("weekly-sync")).unwrap();
        std::fs::create_dir(dir.path().join("weekly-sync-2")).unwrap();
        std::fs::create_dir(dir.path().join("weekly-sync-3")).unwrap();

        let result = unique_dir(dir.path(), "weekly-sync");
        assert_eq!(result, dir.path().join("weekly-sync-4"));
    }

    #[test]
    fn unique_dir_treats_files_as_occupied_too() {
        // A file (not just a directory) at the candidate path also counts as
        // a collision, since creating a directory there would fail either way.
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("weekly-sync"), b"not a dir").unwrap();

        let result = unique_dir(dir.path(), "weekly-sync");
        assert_eq!(result, dir.path().join("weekly-sync-2"));
    }

    #[test]
    fn export_audio_filename_uses_ogg_extension() {
        let mut meeting = sample_meeting("m1");
        meeting.title = "Weekly Sync".to_string();
        meeting.started_at = "2026-07-09T10:00:00Z".parse().unwrap();
        assert_eq!(
            export_audio_filename(&meeting),
            "2026-07-09-weekly-sync.ogg"
        );
    }

    #[test]
    fn unique_file_returns_path_when_free() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("weekly-sync.ogg");
        assert_eq!(unique_file(&path), path);
    }

    #[test]
    fn unique_file_appends_suffix_on_collision() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("weekly-sync.ogg"), b"taken").unwrap();
        assert_eq!(
            unique_file(&dir.path().join("weekly-sync.ogg")),
            dir.path().join("weekly-sync-2.ogg")
        );
    }

    #[test]
    fn copy_audio_sessions_rejects_empty_sources() {
        let dir = tempfile::TempDir::new().unwrap();
        let err = copy_audio_sessions(&[], &dir.path().join("out.ogg")).unwrap_err();
        assert!(err.contains("No recorded audio"));
    }

    #[test]
    fn copy_audio_sessions_writes_a_single_file_to_dest() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = dir.path().join("0.ogg");
        std::fs::write(&src, b"ogg-bytes").unwrap();
        let dest = dir.path().join("export").join("weekly-sync.ogg");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();

        let written = copy_audio_sessions(&[src], &dest).unwrap();
        assert_eq!(written, vec![dest.clone()]);
        assert_eq!(std::fs::read(&dest).unwrap(), b"ogg-bytes");
    }

    #[test]
    fn copy_audio_sessions_suffixes_multiple_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let src0 = dir.path().join("0.ogg");
        let src1 = dir.path().join("1.ogg");
        std::fs::write(&src0, b"session-0").unwrap();
        std::fs::write(&src1, b"session-1").unwrap();
        let dest = dir.path().join("weekly-sync.ogg");

        let written = copy_audio_sessions(&[src0, src1], &dest).unwrap();
        assert_eq!(
            written,
            vec![
                dir.path().join("weekly-sync-1.ogg"),
                dir.path().join("weekly-sync-2.ogg"),
            ]
        );
        assert_eq!(std::fs::read(&written[0]).unwrap(), b"session-0");
        assert_eq!(std::fs::read(&written[1]).unwrap(), b"session-1");
        assert!(
            !dest.exists(),
            "unsuffixed dest is not written for multi-session"
        );
    }

    // ── markdown ─────────────────────────────────────────────────────────

    #[test]
    fn markdown_includes_metadata_block() {
        let mut meeting = sample_meeting("m1");
        meeting.title = "Weekly Sync".to_string();
        meeting.participants = vec![crate::transcript::MeetingParticipant {
            name: "Alice".to_string(),
            email: None,
            is_organizer: true,
            is_current_user: false,
        }];
        let rendered = render_meeting(&meeting, ExportFormat::Markdown).unwrap();
        assert!(rendered.starts_with("# Weekly Sync\n\n"));
        assert!(rendered.contains("**Date:**"));
        assert!(rendered.contains("**Duration:**"));
        assert!(rendered.contains("**Participants:** Alice"));
        assert!(rendered.contains("**Engine:**"));
        assert!(rendered.contains(&meeting.transcription_profile.engine_label));
    }

    #[test]
    fn markdown_omits_participants_line_when_empty() {
        let meeting = sample_meeting("m1");
        let rendered = render_meeting(&meeting, ExportFormat::Markdown).unwrap();
        assert!(!rendered.contains("**Participants:**"));
    }

    #[test]
    fn markdown_includes_structured_summary_sections() {
        let mut meeting = sample_meeting("m1");
        meeting.summary = Some("Prose summary".to_string());
        meeting.structured_summary = Some(StructuredSummary {
            decisions: vec!["Ship Friday".to_string()],
            action_items: vec![StructuredActionItem {
                text: "Open PR".to_string(),
                owner: Some("Alice".to_string()),
            }],
            open_questions: vec!["Budget approved?".to_string()],
        });
        let rendered = render_meeting(&meeting, ExportFormat::Markdown).unwrap();
        assert!(rendered.contains("## Structured Summary"));
        assert!(rendered.contains("### Decisions"));
        assert!(rendered.contains("- Ship Friday"));
        assert!(rendered.contains("**Alice**: Open PR"));
        assert!(rendered.contains("### Open Questions"));
        assert!(rendered.contains("- Budget approved?"));
    }

    #[test]
    fn markdown_omits_structured_summary_when_empty() {
        let mut meeting = sample_meeting("m1");
        meeting.structured_summary = Some(StructuredSummary::default());
        let rendered = render_meeting(&meeting, ExportFormat::Markdown).unwrap();
        assert!(!rendered.contains("## Structured Summary"));
    }

    #[test]
    fn markdown_includes_notes_and_summary_sections_when_present() {
        let mut meeting = sample_meeting("m1");
        meeting.notes = Some("Remember the budget question".to_string());
        meeting.summary = Some("- Point one\n- Point two".to_string());
        let rendered = render_meeting(&meeting, ExportFormat::Markdown).unwrap();
        assert!(rendered.contains("## Notes\n\nRemember the budget question"));
        assert!(rendered.contains("## Summary\n\n- Point one\n- Point two"));
    }

    #[test]
    fn markdown_omits_notes_and_summary_sections_when_absent() {
        let meeting = sample_meeting("m1");
        let rendered = render_meeting(&meeting, ExportFormat::Markdown).unwrap();
        assert!(!rendered.contains("## Notes"));
        assert!(!rendered.contains("## Summary"));
    }

    #[test]
    fn markdown_edited_transcript_takes_precedence_over_segments() {
        let mut meeting = sample_meeting("m1");
        meeting.edited_transcript = Some("This is the cleaned-up transcript.".to_string());
        let rendered = render_meeting(&meeting, ExportFormat::Markdown).unwrap();
        assert!(rendered.contains("## Transcript\n\nThis is the cleaned-up transcript."));
        // The raw segment text ("Hello world") must not also appear as a
        // paragraph; only the edited text represents the transcript.
        assert!(!rendered.contains("[0:00] Hello world"));
    }

    // ── render_transcript_text (meeting detail "Copy") ──────────────────

    #[test]
    fn transcript_text_uses_edited_transcript_verbatim() {
        let mut meeting = sample_meeting("m1");
        meeting.edited_transcript = Some("This is the cleaned-up transcript.".to_string());
        assert_eq!(
            render_transcript_text(&meeting),
            "This is the cleaned-up transcript."
        );
    }

    #[test]
    fn transcript_text_falls_back_to_paragraphs_without_notes_or_summary() {
        let meeting = sample_meeting("m1");
        let text = render_transcript_text(&meeting);
        assert!(text.contains("[0:00] Hello world"));
        assert!(!text.contains("## "));
    }

    #[test]
    fn markdown_falls_back_to_paragraphs_when_edited_transcript_is_blank() {
        let mut meeting = sample_meeting("m1");
        meeting.edited_transcript = Some("   ".to_string());
        let rendered = render_meeting(&meeting, ExportFormat::Markdown).unwrap();
        assert!(rendered.contains("[0:00] Hello world"));
    }

    #[test]
    fn markdown_paragraphs_use_speaker_prefix_when_diarized() {
        let mut meeting = sample_meeting("m1");
        meeting.segments = vec![
            diarized_segment("Hi there", 0.0, 1.0, Speaker::Me),
            diarized_segment("Hello back", 2.0, 3.0, Speaker::Them),
        ];
        let rendered = render_meeting(&meeting, ExportFormat::Markdown).unwrap();
        assert!(rendered.contains("**Me** [0:00] Hi there"));
        assert!(rendered.contains("**Them** [0:02] Hello back"));
    }

    #[test]
    fn markdown_paragraphs_have_no_speaker_prefix_when_not_diarized() {
        let meeting = sample_meeting("m1");
        let rendered = render_meeting(&meeting, ExportFormat::Markdown).unwrap();
        assert!(rendered.contains("[0:00] Hello world"));
        assert!(!rendered.contains("**Me**"));
        assert!(!rendered.contains("**Them**"));
    }

    // ── json ─────────────────────────────────────────────────────────────

    #[test]
    fn json_round_trips_the_full_meeting() {
        let meeting = sample_meeting("m1");
        let rendered = render_meeting(&meeting, ExportFormat::Json).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(parsed["id"], "m1");
        assert_eq!(parsed["segments"][0]["text"], "Hello world");
        // Pretty-printed: multi-line, not a single compact line.
        assert!(rendered.contains('\n'));
    }

    // ── srt ──────────────────────────────────────────────────────────────

    #[test]
    fn srt_renders_standard_blocks() {
        let mut meeting = sample_meeting("m1");
        meeting.segments = vec![sample_segment("Hello world", 0.0, 1.5)];
        let rendered = render_meeting(&meeting, ExportFormat::Srt).unwrap();
        assert_eq!(rendered, "1\n00:00:00,000 --> 00:00:01,500\nHello world\n");
    }

    #[test]
    fn srt_skips_empty_text_segments_and_renumbers() {
        let mut meeting = sample_meeting("m1");
        meeting.segments = vec![
            sample_segment("First", 0.0, 1.0),
            sample_segment("   ", 1.0, 2.0),
            sample_segment("Second", 2.0, 3.0),
        ];
        let rendered = render_meeting(&meeting, ExportFormat::Srt).unwrap();
        assert!(rendered.starts_with("1\n"));
        assert!(rendered.contains("\n2\n"));
        assert!(!rendered.contains("3\n"));
    }

    #[test]
    fn srt_uses_speaker_prefix_when_diarized() {
        let mut meeting = sample_meeting("m1");
        meeting.segments = vec![
            diarized_segment("Hi there", 0.0, 1.0, Speaker::Me),
            diarized_segment("Hello back", 1.0, 2.0, Speaker::Them),
        ];
        let rendered = render_meeting(&meeting, ExportFormat::Srt).unwrap();
        assert!(rendered.contains("Me: Hi there"));
        assert!(rendered.contains("Them: Hello back"));
    }

    #[test]
    fn srt_formats_timestamps_past_one_hour() {
        let mut meeting = sample_meeting("m1");
        meeting.segments = vec![sample_segment("Late in the meeting", 3_661.25, 3_662.5)];
        let rendered = render_meeting(&meeting, ExportFormat::Srt).unwrap();
        assert!(rendered.contains("01:01:01,250 --> 01:01:02,500"));
    }

    #[test]
    fn srt_clamps_zero_or_inverted_duration_to_minimum() {
        let mut meeting = sample_meeting("m1");
        // end_time == start_time
        meeting.segments = vec![sample_segment("Instant", 10.0, 10.0)];
        let rendered = render_meeting(&meeting, ExportFormat::Srt).unwrap();
        assert!(rendered.contains("00:00:10,000 --> 00:00:10,500"));
    }

    #[test]
    fn srt_clamps_inverted_end_before_start() {
        let mut meeting = sample_meeting("m1");
        meeting.segments = vec![sample_segment("Bad data", 10.0, 5.0)];
        let rendered = render_meeting(&meeting, ExportFormat::Srt).unwrap();
        assert!(rendered.contains("00:00:10,000 --> 00:00:10,500"));
    }

    #[test]
    fn srt_sorts_diarized_interleaved_segments_by_start_time() {
        let mut meeting = sample_meeting("m1");
        // Storage order interleaves Me/Them per frame, not by time.
        meeting.segments = vec![
            diarized_segment("Second line", 5.0, 6.0, Speaker::Them),
            diarized_segment("First line", 1.0, 2.0, Speaker::Me),
            diarized_segment("Third line", 8.0, 9.0, Speaker::Me),
        ];
        let rendered = render_meeting(&meeting, ExportFormat::Srt).unwrap();
        let expected = "1\n\
             00:00:01,000 --> 00:00:02,000\n\
             Me: First line\n\
             \n\
             2\n\
             00:00:05,000 --> 00:00:06,000\n\
             Them: Second line\n\
             \n\
             3\n\
             00:00:08,000 --> 00:00:09,000\n\
             Me: Third line\n";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn srt_keeps_storage_order_for_non_diarized_segments() {
        let mut meeting = sample_meeting("m1");
        // Legacy window-relative timestamps: storage order is authoritative
        // and must not be reordered even though start times go backwards.
        meeting.segments = vec![
            sample_segment("Window one", 4.0, 4.5),
            sample_segment("Window two", 0.2, 0.7),
        ];
        let rendered = render_meeting(&meeting, ExportFormat::Srt).unwrap();
        let one = rendered.find("Window one").unwrap();
        let two = rendered.find("Window two").unwrap();
        assert!(one < two);
    }

    // ── vtt ──────────────────────────────────────────────────────────────

    #[test]
    fn vtt_starts_with_webvtt_header() {
        let meeting = sample_meeting("m1");
        let rendered = render_meeting(&meeting, ExportFormat::Vtt).unwrap();
        assert!(rendered.starts_with("WEBVTT\n\n"));
    }

    #[test]
    fn vtt_uses_dot_decimal_separator() {
        let mut meeting = sample_meeting("m1");
        meeting.segments = vec![sample_segment("Hello world", 0.0, 1.5)];
        let rendered = render_meeting(&meeting, ExportFormat::Vtt).unwrap();
        assert!(rendered.contains("00:00:00.000 --> 00:00:01.500"));
        assert!(!rendered.contains(','));
    }

    #[test]
    fn vtt_sorts_diarized_interleaved_segments_by_start_time() {
        let mut meeting = sample_meeting("m1");
        meeting.segments = vec![
            diarized_segment("Second line", 5.0, 6.0, Speaker::Them),
            diarized_segment("First line", 1.0, 2.0, Speaker::Me),
            diarized_segment("Third line", 8.0, 9.0, Speaker::Me),
        ];
        let rendered = render_meeting(&meeting, ExportFormat::Vtt).unwrap();
        let expected = "WEBVTT\n\
             \n\
             00:00:01.000 --> 00:00:02.000\n\
             Me: First line\n\
             \n\
             00:00:05.000 --> 00:00:06.000\n\
             Them: Second line\n\
             \n\
             00:00:08.000 --> 00:00:09.000\n\
             Me: Third line\n";
        assert_eq!(rendered, expected);
    }
}
