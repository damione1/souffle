use crate::engine::{Speaker, TranscriptionSegment};
use crate::export::{PARAGRAPH_PAUSE_THRESHOLD_SECONDS, paragraphs};

/// Format one grouped paragraph as a labeled turn for the summary LLM.
fn format_turn(paragraph: &paragraphs::Paragraph) -> String {
    match paragraph.speaker {
        Some(Speaker::Me) => format!("[{}] Me: {}", paragraph.timestamp, paragraph.text),
        Some(Speaker::Them) => format!("[{}] Them: {}", paragraph.timestamp, paragraph.text),
        None => format!("[{}] {}", paragraph.timestamp, paragraph.text),
    }
}

/// Speaker-labeled turns with timestamps, using the same grouping as the
/// transcript UI / Markdown export (`paragraphs.ts`).
pub fn turns_from_segments(segments: &[TranscriptionSegment]) -> Vec<String> {
    let nonempty: Vec<TranscriptionSegment> = segments
        .iter()
        .filter(|segment| !segment.text.trim().is_empty())
        .cloned()
        .collect();
    paragraphs::group_into_paragraphs(&nonempty, PARAGRAPH_PAUSE_THRESHOLD_SECONDS)
        .into_iter()
        .map(|paragraph| format_turn(&paragraph))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::turns_from_segments;
    use crate::engine::Speaker;
    use crate::test_helpers::fixtures::sample_segment;

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
