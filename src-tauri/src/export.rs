//! Renders a `MeetingTranscript` into Markdown, JSON, SRT, or VTT text for
//! the single-meeting export feature. Pure string rendering: no I/O here,
//! `commands::meetings` handles the file dialog / filesystem write.
//!
//! Also hosts the small filesystem-naming helpers (`archive_folder_name`,
//! `unique_dir`) shared by the full-archive export in `crate::archive`, since
//! they follow the same date+slug naming convention as [`export_default_filename`].

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::engine::{Speaker, TranscriptionSegment};
use crate::transcript::{MeetingSpeaker, MeetingTranscript, StructuredSummary};

/// Minimum on-screen duration given to a subtitle cue whose segment has a
/// zero or inverted end time, so SRT/VTT players never render a cue with
/// zero (or negative) length.
const MIN_CUE_DURATION_SECONDS: f64 = 0.5;

/// Paragraph pause threshold used for Markdown transcript rendering; mirrors
/// the `pauseThreshold` the live transcript view passes to
/// `groupIntoParagraphs` (src/lib/utils/paragraphs.ts).
const PARAGRAPH_PAUSE_THRESHOLD_SECONDS: f64 = 1.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type)]
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
    match non_empty(meeting.edited_transcript.as_deref()) {
        Some(edited) => out.push_str(edited),
        None => {
            let grouped = paragraphs::group_into_paragraphs(
                &meeting.segments,
                PARAGRAPH_PAUSE_THRESHOLD_SECONDS,
            );
            let rendered = grouped
                .iter()
                .map(|p| render_paragraph_markdown(p, &meeting.speakers))
                .collect::<Vec<_>>()
                .join("\n\n");
            out.push_str(&rendered);
        }
    }
    out.push('\n');

    out
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

/// Display name for a speaker label: Me -> "Me", Them -> "Them", a
/// persistent speaker -> its `MeetingSpeaker.name` if it's in the list
/// (i.e. still referenced and resolvable), else a "Speaker <id>" fallback.
fn speaker_display_name(speaker: Speaker, speakers: &[MeetingSpeaker]) -> String {
    match speaker {
        Speaker::Me => "Me".to_string(),
        Speaker::Them => "Them".to_string(),
        Speaker::Persistent(id) => speakers
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| format!("Speaker {id}")),
    }
}

fn render_paragraph_markdown(p: &paragraphs::Paragraph, speakers: &[MeetingSpeaker]) -> String {
    match p.speaker {
        Some(speaker) => format!(
            "**{}** [{}] {}",
            speaker_display_name(speaker, speakers),
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
/// legacy window-relative timestamps must not be reordered. Same rule as the
/// TS `groupIntoParagraphs` (src/lib/utils/paragraphs.ts); the sort is stable
/// on both sides, so equal start times preserve storage order too.
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

fn speaker_prefix(speaker: Option<Speaker>, text: &str, speakers: &[MeetingSpeaker]) -> String {
    match speaker {
        Some(speaker) => format!("{}: {text}", speaker_display_name(speaker, speakers)),
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
            speaker_prefix(seg.speaker, text, &meeting.speakers),
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
            speaker_prefix(seg.speaker, text, &meeting.speakers),
        ));
    }
    finish_cue_block(out)
}

/// Trailing blank line between cues is a separator, not part of the file;
/// trim it and end with exactly one newline.
fn finish_cue_block(out: String) -> String {
    format!("{}\n", out.trim_end())
}

/// Rust port of `groupIntoParagraphs` from `src/lib/utils/paragraphs.ts`,
/// used only for the Markdown export's `## Transcript` section. Kept as a
/// private submodule (not re-exported) since nothing outside export
/// rendering needs paragraph grouping on the backend. See
/// `src-tauri/tests/fixtures/paragraph_grouping.json` for the cross-language
/// fixture that pins this port against the TS original.
mod paragraphs {
    use crate::engine::{Speaker, TranscriptionSegment};

    /// Paragraph break after this many sentences, even with no pause.
    pub const MAX_SENTENCES_PER_PARAGRAPH: usize = 4;
    /// Once a paragraph reaches this length, break at the next sentence end.
    pub const SOFT_MAX_CHARS: usize = 480;
    /// Absolute ceiling for streams with no punctuation at all.
    pub const HARD_MAX_CHARS: usize = 700;

    /// Closing quote/bracket characters allowed between sentence-ending
    /// punctuation and the sentence boundary itself, mirroring the TS
    /// `["»”')\]]` character class.
    const CLOSING_CHARS: [char; 6] = ['"', '»', '\u{201d}', '\'', ')', ']'];
    const SENTENCE_END_CHARS: [char; 4] = ['.', '!', '?', '\u{2026}'];

    #[derive(Debug, Clone, PartialEq)]
    pub struct Paragraph {
        pub timestamp: String,
        pub text: String,
        pub speaker: Option<Speaker>,
    }

    fn format_timestamp(seconds: f64) -> String {
        let mins = (seconds / 60.0).floor() as i64;
        let secs = (seconds % 60.0).floor() as i64;
        format!("{mins}:{secs:02}")
    }

    /// Does `text` end with sentence-ending punctuation, optionally followed
    /// by closing quotes/brackets? Port of the TS `SENTENCE_END` regex
    /// (`/[.!?…]["»”')\]]*\s*$/`); `text` is expected pre-trimmed of
    /// whitespace like the TS caller, but trailing whitespace is stripped
    /// defensively to match the regex's `\s*$` exactly.
    fn ends_sentence(text: &str) -> bool {
        let mut chars: Vec<char> = text.chars().collect();
        while matches!(chars.last(), Some(c) if c.is_whitespace()) {
            chars.pop();
        }
        while matches!(chars.last(), Some(c) if CLOSING_CHARS.contains(c)) {
            chars.pop();
        }
        matches!(chars.last(), Some(c) if SENTENCE_END_CHARS.contains(c))
    }

    /// Count sentence-ending punctuation runs in `text` that are followed by
    /// whitespace or end-of-string (after skipping closing quotes/brackets).
    /// Port of the TS `countSentenceEnds`, which uses a lookahead the `regex`
    /// crate doesn't support, so this walks the string manually instead.
    fn count_sentence_ends(text: &str) -> usize {
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let mut count = 0;
        let mut i = 0;
        while i < n {
            if SENTENCE_END_CHARS.contains(&chars[i]) {
                let mut j = i + 1;
                while j < n && SENTENCE_END_CHARS.contains(&chars[j]) {
                    j += 1;
                }
                let mut k = j;
                while k < n && CLOSING_CHARS.contains(&chars[k]) {
                    k += 1;
                }
                if k == n || chars[k].is_whitespace() {
                    count += 1;
                }
                i = j;
            } else {
                i += 1;
            }
        }
        count
    }

    /// Cluster time-sorted diarized segments into per-speaker turns, then
    /// emit them ordered by each turn's start time (turn segments stay
    /// contiguous and chronological internally). A segment joins its
    /// speaker's currently open turn if the gap since that turn's last
    /// segment is under `pause_threshold`; otherwise that speaker's turn
    /// closes and a new one opens. Port of `clusterIntoTurns` in
    /// `src/lib/utils/paragraphs.ts`; a `HashMap<Speaker, usize>` of open
    /// turn indices stands in for the TS `Map`, generalizing to any number of
    /// speakers (not just Me/Them).
    ///
    /// Without a pause, a monologue would otherwise absorb everything
    /// indefinitely. Opening a new turn for speaker B:
    /// - If B starts clearly after A's last end (>= 350ms handoff), A's turn
    ///   closes immediately so A's later speech opens a fresh line below.
    /// - If B overlaps A or starts within 350ms (crosstalk / tight
    ///   interjection), A is marked interrupted and keeps absorbing until a
    ///   sentence end.
    fn cluster_into_turns<'a>(
        sorted: Vec<&'a TranscriptionSegment>,
        pause_threshold: f64,
    ) -> Vec<&'a TranscriptionSegment> {
        const HANDOFF_GAP_S: f64 = 0.35;
        struct Turn<'a> {
            start: f64,
            last_end: f64,
            segments: Vec<&'a TranscriptionSegment>,
            interrupted: bool,
        }

        let mut open_turns: std::collections::HashMap<Speaker, usize> =
            std::collections::HashMap::new();
        let mut turns: Vec<Turn<'a>> = Vec::new();

        for seg in sorted {
            let end = if seg.end_time != 0.0 { seg.end_time } else { seg.start_time };
            let Some(speaker) = seg.speaker else {
                turns.push(Turn { start: seg.start_time, last_end: end, segments: vec![seg], interrupted: false });
                continue;
            };

            let open_idx = open_turns.get(&speaker).copied();
            let joins = match open_idx {
                Some(idx) => seg.start_time - turns[idx].last_end < pause_threshold,
                None => false,
            };
            if let Some(idx) = open_idx.filter(|_| joins) {
                turns[idx].segments.push(seg);
                turns[idx].last_end = turns[idx].last_end.max(end);
                if turns[idx].interrupted && ends_sentence(seg.text.trim()) {
                    // First sentence end at or after the interruption: close
                    // now so the speaker's next segment starts a fresh,
                    // later-sorting turn.
                    open_turns.remove(&speaker);
                }
            } else {
                turns.push(Turn { start: seg.start_time, last_end: end, segments: vec![seg], interrupted: false });
                let new_idx = turns.len() - 1;
                open_turns.insert(speaker, new_idx);
                let handoffs: Vec<(Speaker, usize)> = open_turns
                    .iter()
                    .filter(|(other, _)| **other != speaker)
                    .map(|(&other, &idx)| (other, idx))
                    .collect();
                for (other_speaker, idx) in handoffs {
                    if turns[new_idx].start >= turns[idx].last_end + HANDOFF_GAP_S {
                        open_turns.remove(&other_speaker);
                    } else {
                        turns[idx].interrupted = true;
                    }
                }
            }
        }

        turns.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
        turns.into_iter().flat_map(|t| t.segments).collect()
    }

    fn flush_paragraph(
        paragraphs: &mut Vec<Paragraph>,
        timestamp: &str,
        speaker: Option<Speaker>,
        words: &mut Vec<String>,
    ) {
        paragraphs.push(Paragraph {
            timestamp: timestamp.to_string(),
            text: words.join(" "),
            speaker,
        });
        words.clear();
    }

    /// Group segments into flowing paragraphs with a leading timestamp. See
    /// `groupIntoParagraphs` in `src/lib/utils/paragraphs.ts` for the
    /// authoritative rule set (pause threshold, sentence/char caps, diarized
    /// sort-then-cluster-into-turns): this is a line-for-line port,
    /// including its quirk of seeding the first paragraph's timestamp from
    /// `ordered[0]` even if that segment's text turns out to be empty.
    pub fn group_into_paragraphs(
        segments: &[TranscriptionSegment],
        pause_threshold: f64,
    ) -> Vec<Paragraph> {
        if segments.is_empty() {
            return Vec::new();
        }

        let diarized = segments.iter().any(|s| s.speaker.is_some());
        let time_ordered = super::time_ordered_segments(segments);
        let ordered = if diarized {
            cluster_into_turns(time_ordered, pause_threshold)
        } else {
            time_ordered
        };

        let mut paragraphs: Vec<Paragraph> = Vec::new();
        let mut current_timestamp = format_timestamp(ordered[0].start_time);
        let mut current_speaker: Option<Speaker> = ordered[0].speaker;
        let mut current_words: Vec<String> = Vec::new();
        let mut current_chars: usize = 0;
        let mut sentence_count: usize = 0;
        let mut ends_sentence_flag = false;
        let mut last_end = ordered[0].start_time;

        for seg in &ordered {
            let text = seg.text.trim();
            if text.is_empty() {
                continue;
            }
            let speaker = seg.speaker;

            if !current_words.is_empty() {
                let mut broke = false;
                if diarized && speaker != current_speaker {
                    flush_paragraph(
                        &mut paragraphs,
                        &current_timestamp,
                        current_speaker,
                        &mut current_words,
                    );
                    broke = true;
                } else {
                    let gap = seg.start_time - last_end;
                    let break_at_sentence = ends_sentence_flag
                        && (gap >= pause_threshold
                            || sentence_count >= MAX_SENTENCES_PER_PARAGRAPH
                            || current_chars >= SOFT_MAX_CHARS);
                    let break_hard = current_chars >= HARD_MAX_CHARS;

                    if break_at_sentence || break_hard {
                        flush_paragraph(
                            &mut paragraphs,
                            &current_timestamp,
                            current_speaker,
                            &mut current_words,
                        );
                        broke = true;
                    }
                }

                if broke {
                    current_timestamp = format_timestamp(seg.start_time);
                    current_speaker = speaker;
                    current_chars = 0;
                    sentence_count = 0;
                    // ends_sentence_flag is unconditionally recomputed below
                    // from this segment's text, so no reset needed here.
                }
            } else {
                current_speaker = speaker;
            }

            current_words.push(text.to_string());
            current_chars += text.chars().count() + 1;
            sentence_count += count_sentence_ends(text);
            ends_sentence_flag = ends_sentence(text);
            let end = if seg.end_time != 0.0 {
                seg.end_time
            } else {
                seg.start_time
            };
            last_end = last_end.max(end);
        }

        if !current_words.is_empty() {
            paragraphs.push(Paragraph {
                timestamp: current_timestamp,
                text: current_words.join(" "),
                speaker: current_speaker,
            });
        }

        paragraphs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::StructuredActionItem;
    use crate::test_helpers::fixtures::{sample_meeting, sample_segment};

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
    fn markdown_paragraphs_resolve_persistent_speaker_name() {
        let mut meeting = sample_meeting("m1");
        meeting.segments = vec![diarized_segment("Hi there", 0.0, 1.0, Speaker::Persistent(1))];
        meeting.speakers = vec![crate::transcript::MeetingSpeaker {
            id: 1,
            name: "Alice".to_string(),
        }];
        let rendered = render_meeting(&meeting, ExportFormat::Markdown).unwrap();
        assert!(rendered.contains("**Alice** [0:00] Hi there"));
    }

    #[test]
    fn markdown_paragraphs_fall_back_to_speaker_id_when_unresolved() {
        let mut meeting = sample_meeting("m1");
        meeting.segments = vec![diarized_segment("Hi there", 0.0, 1.0, Speaker::Persistent(99))];
        // No matching entry in meeting.speakers (deleted, or never resolved).
        let rendered = render_meeting(&meeting, ExportFormat::Markdown).unwrap();
        assert!(rendered.contains("**Speaker 99** [0:00] Hi there"));
    }

    #[test]
    fn srt_resolves_persistent_speaker_name() {
        let mut meeting = sample_meeting("m1");
        meeting.segments = vec![diarized_segment("Hi there", 0.0, 1.0, Speaker::Persistent(7))];
        meeting.speakers = vec![crate::transcript::MeetingSpeaker {
            id: 7,
            name: "Bob".to_string(),
        }];
        let rendered = render_meeting(&meeting, ExportFormat::Srt).unwrap();
        assert!(rendered.contains("Bob: Hi there"));
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

    // ── turn interruption (crosstalk vs. monologue) ─────────────────────

    #[test]
    fn interrupted_turn_closes_at_the_following_sentence_end() {
        // Me monologues without a pause; Them interjects mid-monologue. Me's
        // turn must not absorb everything: it closes at the first sentence
        // end after the interjection, so Them's turn sorts in between Me's
        // two turns instead of trailing behind the whole monologue.
        let segments = vec![
            diarized_segment("Let me explain the whole plan", 0.0, 0.5, Speaker::Me),
            diarized_segment("in detail because it's", 0.6, 1.1, Speaker::Me),
            diarized_segment("wait", 1.2, 1.7, Speaker::Them),
            diarized_segment("complicated.", 1.8, 2.3, Speaker::Me),
            diarized_segment("So let's start now", 3.0, 3.5, Speaker::Me),
        ];
        let result = paragraphs::group_into_paragraphs(&segments, 1.5);
        let texts: Vec<(Option<Speaker>, &str)> =
            result.iter().map(|p| (p.speaker, p.text.as_str())).collect();
        assert_eq!(
            texts,
            vec![
                (Some(Speaker::Me), "Let me explain the whole plan in detail because it's complicated."),
                (Some(Speaker::Them), "wait"),
                (Some(Speaker::Me), "So let's start now"),
            ]
        );
    }

    #[test]
    fn interrupted_turn_ignores_a_sentence_end_before_the_interruption() {
        // Me already finished a sentence before Them interjects; that
        // earlier sentence end must not retroactively split the turn, only
        // the one that comes after the interjection does.
        let segments = vec![
            diarized_segment("First point.", 0.0, 0.5, Speaker::Me),
            diarized_segment("Second part continues", 0.6, 1.1, Speaker::Me),
            diarized_segment("quick question", 1.2, 1.7, Speaker::Them),
            diarized_segment("and concludes.", 1.8, 2.3, Speaker::Me),
            diarized_segment("New topic starts", 3.0, 3.5, Speaker::Me),
        ];
        let result = paragraphs::group_into_paragraphs(&segments, 1.5);
        let texts: Vec<(Option<Speaker>, &str)> =
            result.iter().map(|p| (p.speaker, p.text.as_str())).collect();
        assert_eq!(
            texts,
            vec![
                (Some(Speaker::Me), "First point. Second part continues and concludes."),
                (Some(Speaker::Them), "quick question"),
                (Some(Speaker::Me), "New topic starts"),
            ]
        );
    }

    #[test]
    fn unpunctuated_sequential_handoff_opens_a_new_line() {
        // Me never produces sentence-final punctuation. Them starts clearly
        // after Me's last end (>= 350ms handoff), so Me closes immediately
        // and later Me speech opens a fresh turn below.
        let segments = vec![
            diarized_segment("so basically", 0.0, 0.5, Speaker::Me),
            diarized_segment("we were thinking", 0.6, 1.1, Speaker::Me),
            diarized_segment("right", 1.6, 2.1, Speaker::Them),
            diarized_segment("about moving the launch date", 2.2, 2.7, Speaker::Me),
            diarized_segment("to next quarter", 2.8, 3.3, Speaker::Me),
        ];
        let result = paragraphs::group_into_paragraphs(&segments, 1.5);
        let texts: Vec<(Option<Speaker>, &str)> =
            result.iter().map(|p| (p.speaker, p.text.as_str())).collect();
        assert_eq!(
            texts,
            vec![
                (Some(Speaker::Me), "so basically we were thinking"),
                (Some(Speaker::Them), "right"),
                (Some(Speaker::Me), "about moving the launch date to next quarter"),
            ]
        );
    }

    // ── cross-language paragraph fixture ────────────────────────────────

    #[derive(serde::Deserialize)]
    struct FixtureSegment {
        text: String,
        start_time: f64,
        end_time: f64,
        speaker: Option<Speaker>,
    }

    #[derive(serde::Deserialize)]
    struct FixtureParagraph {
        timestamp: String,
        text: String,
        speaker: Option<Speaker>,
    }

    #[derive(serde::Deserialize)]
    struct FixtureCase {
        name: String,
        segments: Vec<FixtureSegment>,
        expected: Vec<FixtureParagraph>,
    }

    #[derive(serde::Deserialize)]
    struct Fixture {
        pause_threshold: f64,
        cases: Vec<FixtureCase>,
    }

    #[test]
    fn paragraph_grouping_matches_the_typescript_reference() {
        let raw = include_str!("../tests/fixtures/paragraph_grouping.json");
        let fixture: Fixture = serde_json::from_str(raw).expect("valid fixture JSON");

        for case in &fixture.cases {
            let segments: Vec<crate::engine::TranscriptionSegment> = case
                .segments
                .iter()
                .map(|s| crate::engine::TranscriptionSegment {
                    text: s.text.clone(),
                    start_time: s.start_time,
                    end_time: s.end_time,
                    is_final: true,
                    language: None,
                    confidence: None,
                    speaker: s.speaker,
                })
                .collect();

            let result = paragraphs::group_into_paragraphs(&segments, fixture.pause_threshold);

            assert_eq!(
                result.len(),
                case.expected.len(),
                "case '{}': paragraph count mismatch",
                case.name
            );
            for (actual, expected) in result.iter().zip(case.expected.iter()) {
                assert_eq!(
                    actual.timestamp, expected.timestamp,
                    "case '{}': timestamp mismatch",
                    case.name
                );
                assert_eq!(
                    actual.text, expected.text,
                    "case '{}': text mismatch",
                    case.name
                );
                assert_eq!(
                    actual.speaker, expected.speaker,
                    "case '{}': speaker mismatch",
                    case.name
                );
            }
        }
    }
}
