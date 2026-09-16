//! Port of `buildMeetingTranscriptBlocks`/`groupIntoParagraphsWithRanges`
//! (`src/lib/utils/paragraphs.ts`) for a completed meeting's transcript -
//! milestone 8a. Deliberately a simplified grouping rule, not the exact
//! algorithm: the real one clusters into speaker "turns" first, then splits
//! each turn into paragraphs by pause/sentence-count/char-length caps, with
//! extra handoff/interrupt/crosstalk timing heuristics on top. Reproducing
//! that exactly is a substantial, separate port; this groups consecutive
//! segments by pause threshold and speaker continuity only - real
//! paragraphs with real speaker labels and real session breaks, just not
//! byte-identical boundaries to the Svelte reference. Said here rather than
//! silently approximated as done.

use souffle_lib::engine::{Speaker, TranscriptionSegment};
use souffle_lib::transcript::MeetingRecordingSession;

use crate::TranscriptBlock;

fn speaker_label(speaker: Option<Speaker>) -> String {
    match speaker {
        Some(Speaker::Me) => "Moi".into(),
        Some(Speaker::Them) => "Eux".into(),
        None => String::new(),
    }
}

fn seg_end(seg: &TranscriptionSegment) -> f64 {
    if seg.end_time > 0.0 {
        seg.end_time
    } else {
        seg.start_time
    }
}

/// Groups one contiguous run of segments (already known to belong to a
/// single recording session, or none) into paragraph blocks: a new
/// paragraph starts when the pause since the previous segment exceeds
/// `pause_threshold`, or the speaker changes.
fn group_paragraphs(
    segments: &[TranscriptionSegment],
    segment_offset: usize,
    session_index: Option<usize>,
    pause_threshold: f64,
) -> Vec<TranscriptBlock> {
    let mut blocks = Vec::new();
    let mut current: Vec<&TranscriptionSegment> = Vec::new();
    let mut last_end = 0.0f64;

    let flush = |current: &mut Vec<&TranscriptionSegment>, blocks: &mut Vec<TranscriptBlock>| {
        if current.is_empty() {
            return;
        }
        let text = current
            .iter()
            .map(|s| s.text.trim())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let first = current[0];
        blocks.push(TranscriptBlock {
            is_session_break: false,
            speaker_label: speaker_label(first.speaker).into(),
            timestamp: crate::timeline::format_duration(first.start_time).into(),
            text: text.into(),
            recording_session_index: session_index.map(|i| i as i32).unwrap_or(-1),
            start_time: first.start_time as f32,
            end_label: "".into(),
            start_label: "".into(),
        });
        current.clear();
    };

    for (i, seg) in segments.iter().enumerate() {
        let text_offset = segment_offset + i;
        let _ = text_offset; // segment ranges aren't tracked in this simplified port
        let starts_new = if current.is_empty() {
            true
        } else {
            let gap = seg.start_time - last_end;
            let speaker_changed = current.last().unwrap().speaker != seg.speaker;
            gap > pause_threshold || speaker_changed
        };
        if starts_new {
            flush(&mut current, &mut blocks);
        }
        last_end = seg_end(seg);
        current.push(seg);
    }
    flush(&mut current, &mut blocks);
    blocks
}

fn session_break_block() -> TranscriptBlock {
    TranscriptBlock {
        is_session_break: true,
        speaker_label: "".into(),
        timestamp: "".into(),
        text: "".into(),
        recording_session_index: -1,
        start_time: 0.0,
        end_label: "Fin de l'enregistrement pr\u{e9}c\u{e9}dent".into(),
        start_label: "Nouvelle session d'enregistrement".into(),
    }
}

fn push_range(
    blocks: &mut Vec<TranscriptBlock>,
    segments: &[TranscriptionSegment],
    start: usize,
    end: usize,
    session_index: Option<usize>,
    pause_threshold: f64,
    appended_any: &mut bool,
) {
    if end <= start {
        return;
    }
    if *appended_any {
        blocks.push(session_break_block());
    }
    blocks.extend(group_paragraphs(
        &segments[start..end],
        start,
        session_index,
        pause_threshold,
    ));
    *appended_any = true;
}

/// `pauseThreshold` in the Svelte reference is a constant 1.5s.
const PAUSE_THRESHOLD_SECONDS: f64 = 1.5;

pub fn build_transcript_blocks(
    segments: &[TranscriptionSegment],
    recording_sessions: &[MeetingRecordingSession],
) -> Vec<TranscriptBlock> {
    if segments.is_empty() {
        return Vec::new();
    }

    let mut sessions: Vec<(usize, usize, usize)> = recording_sessions
        .iter()
        .enumerate()
        .map(|(i, s)| {
            (
                i,
                (s.start_segment_index as usize).min(segments.len()),
                (s.end_segment_index as usize).min(segments.len()),
            )
        })
        .filter(|(_, start, end)| end > start)
        .collect();
    sessions.sort_by_key(|(_, start, _)| *start);

    let mut blocks = Vec::new();
    let mut consumed_until = 0usize;
    let mut appended_any = false;
    for (session_index, start, end) in sessions {
        let start = start.max(consumed_until);
        if start > consumed_until {
            push_range(
                &mut blocks,
                segments,
                consumed_until,
                start,
                None,
                PAUSE_THRESHOLD_SECONDS,
                &mut appended_any,
            );
        }
        push_range(
            &mut blocks,
            segments,
            start,
            end,
            Some(session_index),
            PAUSE_THRESHOLD_SECONDS,
            &mut appended_any,
        );
        consumed_until = consumed_until.max(end);
    }
    if consumed_until < segments.len() {
        push_range(
            &mut blocks,
            segments,
            consumed_until,
            segments.len(),
            None,
            PAUSE_THRESHOLD_SECONDS,
            &mut appended_any,
        );
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, start: f64, end: f64, speaker: Option<Speaker>) -> TranscriptionSegment {
        TranscriptionSegment {
            text: text.into(),
            start_time: start,
            end_time: end,
            is_final: true,
            language: None,
            confidence: None,
            speaker,
        }
    }

    #[test]
    fn empty_segments_produce_no_blocks() {
        assert!(build_transcript_blocks(&[], &[]).is_empty());
    }

    #[test]
    fn groups_close_segments_into_one_paragraph() {
        let segments = vec![
            seg("Bonjour", 0.0, 1.0, None),
            seg("comment ça va", 1.2, 2.0, None),
        ];
        let blocks = build_transcript_blocks(&segments, &[]);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].text.as_str(), "Bonjour comment ça va");
        assert!(!blocks[0].is_session_break);
    }

    #[test]
    fn splits_on_pause_and_on_speaker_change() {
        let segments = vec![
            seg("Un", 0.0, 1.0, Some(Speaker::Me)),
            seg("Deux", 5.0, 6.0, Some(Speaker::Me)), // pause > 1.5s
            seg("Trois", 6.1, 7.0, Some(Speaker::Them)), // speaker change
        ];
        let blocks = build_transcript_blocks(&segments, &[]);
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].speaker_label.as_str(), "Moi");
        assert_eq!(blocks[2].speaker_label.as_str(), "Eux");
    }

    #[test]
    fn inserts_session_break_between_recording_sessions() {
        use chrono::Utc;
        let segments = vec![
            seg("Premi\u{e8}re session", 0.0, 1.0, None),
            seg("Deuxi\u{e8}me session", 10.0, 11.0, None),
        ];
        let now = Utc::now();
        let sessions = vec![
            MeetingRecordingSession::completed(
                "s0".into(),
                now,
                now,
                0,
                1, /* end_segment_index */
            ),
            MeetingRecordingSession::completed("s1".into(), now, now, 1, 2),
        ];
        let blocks = build_transcript_blocks(&segments, &sessions);
        assert_eq!(blocks.len(), 3, "paragraph, session-break, paragraph");
        assert!(!blocks[0].is_session_break);
        assert!(blocks[1].is_session_break);
        assert!(!blocks[2].is_session_break);
        assert_eq!(blocks[2].recording_session_index, 1);
    }
}
