use souffle_schema::paragraphs;

use crate::engine::TranscriptionSegment;
use crate::transcript::MeetingRecordingSession;

/// Stable source metadata, never inferred from a compact row's position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TurnOrigin {
    pub session_index: usize,
    pub seconds: f64,
}

#[derive(Debug, Clone)]
pub struct SessionTurn {
    pub line: String,
    pub origin: Option<TurnOrigin>,
}

pub(crate) const SESSION_TAG: &str = "session";

/// Group within recording ranges, in storage order, never across a session
/// boundary. Gaps and ambiguous ranges retain their text but have no seek target.
pub fn turns_from_sessions(
    segments: &[TranscriptionSegment],
    sessions: &[MeetingRecordingSession],
) -> Vec<SessionTurn> {
    let mut ranges: Vec<_> = sessions
        .iter()
        .enumerate()
        .filter_map(|(index, session)| {
            let start = (session.start_segment_index as usize).min(segments.len());
            let end = (session.end_segment_index as usize).min(segments.len());
            (end > start).then_some((index, start, end))
        })
        .collect();
    ranges.sort_by_key(|&(_, start, _)| start);
    let mut result = Vec::new();
    let mut append = |start: usize, end: usize, session: Option<usize>| {
        if end <= start {
            return;
        }
        let verified = session.filter(|&index| {
            ranges
                .iter()
                .all(|&(other, a, b)| other == index || b <= start || a >= end)
        });
        let nonempty: Vec<_> = segments[start..end]
            .iter()
            .filter(|segment| !segment.text.trim().is_empty())
            .cloned()
            .collect();
        for paragraph in
            paragraphs::group_into_paragraphs(&nonempty, paragraphs::PAUSE_THRESHOLD_SECONDS)
        {
            let mut line = format_turn(&paragraph);
            if sessions.len() > 1
                && let Some(index) = verified
            {
                let body_start = line.len() - paragraph.text.len();
                line.insert_str(body_start, &format!("[{SESSION_TAG} {}] ", index + 1));
            }
            let origin = verified
                .filter(|_| paragraph.start_time.is_finite() && paragraph.start_time >= 0.)
                .map(|session_index| TurnOrigin {
                    session_index,
                    seconds: paragraph.start_time,
                });
            result.push(SessionTurn { line, origin });
        }
    };
    let mut consumed = 0;
    for &(index, start, end) in &ranges {
        if start > consumed {
            append(consumed, start, None);
        }
        append(start.max(consumed), end, Some(index));
        consumed = consumed.max(end);
    }
    append(consumed, segments.len(), None);
    result
}

/// Format one grouped paragraph as a labeled turn for the summary LLM.
fn format_turn(paragraph: &paragraphs::Paragraph) -> String {
    match paragraph.speaker {
        Some(speaker) => format!(
            "[{}] {}: {}",
            paragraph.timestamp,
            speaker.display_name(),
            paragraph.text
        ),
        None => format!("[{}] {}", paragraph.timestamp, paragraph.text),
    }
}

/// Speaker-labeled turns with timestamps, using the same grouping as the
/// transcript view and the Markdown export (`souffle_schema::paragraphs`).
pub fn turns_from_segments(segments: &[TranscriptionSegment]) -> Vec<String> {
    let nonempty: Vec<TranscriptionSegment> = segments
        .iter()
        .filter(|segment| !segment.text.trim().is_empty())
        .cloned()
        .collect();
    paragraphs::group_into_paragraphs(&nonempty, paragraphs::PAUSE_THRESHOLD_SECONDS)
        .into_iter()
        .map(|paragraph| format_turn(&paragraph))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::turns_from_segments;
    use crate::engine::Speaker;
    use crate::test_helpers::fixtures::sample_segment;

    #[test]
    fn resumed_sessions_preserve_storage_order_and_never_merge_speakers() {
        let now = chrono::Utc::now();
        let sessions = [
            crate::transcript::MeetingRecordingSession::completed("first".into(), now, now, 0, 2),
            crate::transcript::MeetingRecordingSession::completed("resumed".into(), now, now, 2, 4),
        ];
        let turns = super::turns_from_sessions(
            &[
                tagged("First decision", 120., 121., Speaker::Me),
                tagged("accepted", 122., 123., Speaker::Them),
                tagged("Reversed decision", 0., 1., Speaker::Them),
                tagged("after resuming", 2., 3., Speaker::Me),
            ],
            &sessions,
        );
        assert_eq!(turns.len(), 4);
        assert!(turns[0].line.contains("[session 1] First decision"));
        assert!(turns[2].line.contains("[session 2] Reversed decision"));
        assert_eq!(turns[2].origin.unwrap().session_index, 1);
        assert_eq!(turns[2].origin.unwrap().seconds, 0.);
        let adjacent = super::turns_from_sessions(
            &[
                tagged("A", 0., 1., Speaker::Me),
                tagged("B", 1., 2., Speaker::Me),
            ],
            &[
                crate::transcript::MeetingRecordingSession::completed("a".into(), now, now, 0, 1),
                crate::transcript::MeetingRecordingSession::completed("b".into(), now, now, 1, 2),
            ],
        );
        assert_eq!(adjacent.len(), 2);
    }

    fn tagged(
        text: &str,
        start: f64,
        end: f64,
        speaker: Speaker,
    ) -> crate::engine::TranscriptionSegment {
        let mut seg = sample_segment(text, start, end);
        seg.speaker = Some(speaker);
        seg
    }

    #[test]
    fn diarized_turns_carry_timestamp_and_speaker() {
        let turns = turns_from_segments(&[
            tagged("hello there", 12.0, 14.0, Speaker::Me),
            tagged("hi back", 15.0, 16.0, Speaker::Them),
        ]);
        assert_eq!(
            turns,
            vec![
                "[0:12] Me: hello there".to_string(),
                "[0:15] Them: hi back".to_string(),
            ]
        );
    }

    #[test]
    fn undiarized_turns_keep_timestamp_without_a_speaker() {
        let turns = turns_from_segments(&[sample_segment("just talking", 5.0, 7.0)]);
        assert_eq!(turns, vec!["[0:05] just talking".to_string()]);
    }

    #[test]
    fn consecutive_same_speaker_stays_one_turn() {
        let turns = turns_from_segments(&[
            tagged("hello", 0.0, 1.0, Speaker::Me),
            tagged("there", 1.1, 2.0, Speaker::Me),
        ]);
        assert_eq!(turns, vec!["[0:00] Me: hello there".to_string()]);
    }

    #[test]
    fn empty_segments_are_dropped() {
        let turns = turns_from_segments(&[
            tagged("   ", 0.0, 1.0, Speaker::Me),
            tagged("kept", 2.0, 3.0, Speaker::Them),
        ]);
        assert_eq!(turns, vec!["[0:02] Them: kept".to_string()]);
    }

    #[test]
    fn whitespace_prefix_does_not_steal_the_timestamp() {
        let same_speaker = turns_from_segments(&[
            tagged(" ", 0.0, 1.0, Speaker::Me),
            tagged("kept", 2.0, 3.0, Speaker::Me),
        ]);
        assert_eq!(same_speaker, vec!["[0:02] Me: kept".to_string()]);

        let undiarized = turns_from_segments(&[
            sample_segment(" ", 0.0, 1.0),
            sample_segment("kept", 2.0, 3.0),
        ]);
        assert_eq!(undiarized, vec!["[0:02] kept".to_string()]);
    }
}
