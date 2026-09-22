//! The one paragraph-grouping algorithm every transcript renderer shares:
//! the Markdown export, the summary turns sent to the LLM, the Slint
//! post-meeting view and the MCP sidecar all call [`group_into_paragraphs`].
//! `tests/fixtures/paragraph_grouping.json` pins its output; the consumers
//! prove their [`SegmentLike`] implementations against the same fixture.
//!
//! The live transcript view (`souffle-slint/src/live_transcript.rs`) is the
//! deliberate exception: this algorithm splits a turn based on a *later*
//! interrupter, so it cannot run incrementally without re-cutting paragraphs
//! already on screen. The live view keeps its own simpler append rule and
//! shares only [`PAUSE_THRESHOLD_SECONDS`].

use crate::Speaker;

/// Pause between two segments, in seconds, that opens a new paragraph.
pub const PAUSE_THRESHOLD_SECONDS: f64 = 1.5;

/// Paragraph break after this many sentences, even with no pause.
pub const MAX_SENTENCES_PER_PARAGRAPH: usize = 4;
/// Once a paragraph reaches this length, break at the next sentence end.
pub const SOFT_MAX_CHARS: usize = 480;
/// Absolute ceiling for streams with no punctuation at all.
pub const HARD_MAX_CHARS: usize = 700;

/// Sequential handoff: another speaker starting at least this long after
/// the interrupted speaker's last end closes the turn immediately.
const HANDOFF_GAP_S: f64 = 0.35;
/// Overlap: the interrupted turn closes at the latest this long after the
/// interrupter started, when no sentence end comes first.
const INTERRUPT_HOLD_S: f64 = 1.0;
/// The hold above only applies to turns at least this long (a monologue).
const MONOLOGUE_MIN_S: f64 = 2.0;

/// Closing quote/bracket characters allowed between sentence-ending
/// punctuation and the sentence boundary itself.
const CLOSING_CHARS: [char; 6] = ['"', '»', '\u{201d}', '\'', ')', ']'];
const SENTENCE_END_CHARS: [char; 4] = ['.', '!', '?', '\u{2026}'];

/// What the grouper reads from a transcript segment. Implemented by the
/// app's `TranscriptionSegment` and by the sidecar's SQLite row, so both
/// processes run the same algorithm without the sidecar pulling in the
/// app's engine types.
pub trait SegmentLike {
    fn text(&self) -> &str;
    fn start_time(&self) -> f64;
    fn end_time(&self) -> f64;
    fn speaker(&self) -> Option<Speaker>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct Paragraph {
    /// `m:ss` of `start_time`, the form the Markdown export and the summary
    /// turns print.
    pub timestamp: String,
    pub start_time: f64,
    pub text: String,
    pub speaker: Option<Speaker>,
}

fn format_timestamp(seconds: f64) -> String {
    let mins = (seconds / 60.0).floor() as i64;
    let secs = (seconds % 60.0).floor() as i64;
    format!("{mins}:{secs:02}")
}

/// Does `text` end with sentence-ending punctuation, optionally followed
/// by closing quotes/brackets? `text` is expected pre-trimmed of
/// whitespace, but trailing whitespace is stripped defensively.
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

fn seg_end<S: SegmentLike>(seg: &S) -> f64 {
    if seg.end_time() != 0.0 {
        seg.end_time()
    } else {
        seg.start_time()
    }
}

struct Turn<'a, S> {
    start: f64,
    last_end: f64,
    speaker: Option<Speaker>,
    segments: Vec<&'a S>,
}

/// Cluster diarized segments into per-speaker turns.
///
/// Within a lane, emission order is kept (timestamps can jitter after a
/// KV refresh; time-sorting the same speaker zippers two hypotheses).
/// Overlapping turns from another speaker split the interrupted turn
/// so the interruption can sort between the two halves:
/// - Sequential handoff (>= 350ms after the other speaker's last end):
///   close immediately.
/// - Overlap: close at the earlier of the next sentence end or 1s after
///   the interrupter started, but only when the interrupted turn is long
///   enough to be a monologue.
fn cluster_into_turns<S: SegmentLike>(segments: &[S], pause_threshold: f64) -> Vec<Turn<'_, S>> {
    let mut lane_order: Vec<Speaker> = Vec::new();
    let mut lanes: std::collections::HashMap<Speaker, Vec<&S>> = std::collections::HashMap::new();
    let mut untagged: Vec<&S> = Vec::new();

    for seg in segments {
        match seg.speaker() {
            Some(speaker) => {
                lanes
                    .entry(speaker)
                    .or_insert_with(|| {
                        lane_order.push(speaker);
                        Vec::new()
                    })
                    .push(seg);
            }
            None => untagged.push(seg),
        }
    }

    let mut turns: Vec<Turn<'_, S>> = Vec::new();
    for speaker in lane_order {
        let segs = lanes.get(&speaker).map(Vec::as_slice).unwrap_or(&[]);
        turns.extend(pause_split_lane(segs, speaker, pause_threshold));
    }
    for seg in untagged {
        turns.push(Turn {
            start: seg.start_time(),
            last_end: seg_end(seg),
            speaker: None,
            segments: vec![seg],
        });
    }

    split_interrupted_turns(turns, HANDOFF_GAP_S, INTERRUPT_HOLD_S, MONOLOGUE_MIN_S)
}

fn pause_split_lane<'a, S: SegmentLike>(
    segs: &[&'a S],
    speaker: Speaker,
    pause_threshold: f64,
) -> Vec<Turn<'a, S>> {
    let mut turns: Vec<Turn<'a, S>> = Vec::new();
    for &seg in segs {
        let end = seg_end(seg);
        if let Some(current) = turns.last_mut()
            && seg.start_time() - current.last_end < pause_threshold
        {
            current.segments.push(seg);
            current.last_end = current.last_end.max(end);
        } else {
            turns.push(Turn {
                start: seg.start_time(),
                last_end: end,
                speaker: Some(speaker),
                segments: vec![seg],
            });
        }
    }
    turns
}

fn interrupt_split_index<S: SegmentLike>(
    turn: &Turn<'_, S>,
    interrupter: &Turn<'_, S>,
    handoff_gap_s: f64,
    interrupt_hold_s: f64,
    monologue_min_s: f64,
) -> Option<usize> {
    let spoken_before = turn
        .segments
        .iter()
        .position(|s| s.start_time() >= interrupter.start)
        .unwrap_or(turn.segments.len());
    let last_before_end = if spoken_before > 0 {
        seg_end(turn.segments[spoken_before - 1])
    } else {
        turn.start
    };

    if interrupter.start >= last_before_end + handoff_gap_s {
        return if spoken_before > 0 && spoken_before < turn.segments.len() {
            Some(spoken_before)
        } else {
            None
        };
    }

    let hold_at = interrupter.start + interrupt_hold_s;
    let apply_hold = turn.last_end - turn.start >= monologue_min_s;
    for i in spoken_before..turn.segments.len() {
        let seg = turn.segments[i];
        if ends_sentence(seg.text().trim()) {
            return Some(i + 1);
        }
        if apply_hold && seg.start_time() >= hold_at {
            return Some(i);
        }
    }
    None
}

fn split_interrupted_turns<'a, S: SegmentLike>(
    mut remaining: Vec<Turn<'a, S>>,
    handoff_gap_s: f64,
    interrupt_hold_s: f64,
    monologue_min_s: f64,
) -> Vec<Turn<'a, S>> {
    let mut out: Vec<Turn<'a, S>> = Vec::new();
    while !remaining.is_empty() {
        remaining.sort_by(|a, b| {
            a.start
                .partial_cmp(&b.start)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let turn = remaining.remove(0);
        if turn.speaker.is_none() || turn.segments.is_empty() {
            out.push(turn);
            continue;
        }

        let interrupter_idx = remaining.iter().position(|other| {
            other.speaker.is_some()
                && other.speaker != turn.speaker
                && other.start < turn.last_end
                && other.start >= turn.start
        });

        let Some(interrupter_idx) = interrupter_idx else {
            out.push(turn);
            continue;
        };

        let split_at = interrupt_split_index(
            &turn,
            &remaining[interrupter_idx],
            handoff_gap_s,
            interrupt_hold_s,
            monologue_min_s,
        );
        let Some(split_at) = split_at else {
            out.push(turn);
            continue;
        };
        if split_at == 0 || split_at >= turn.segments.len() {
            out.push(turn);
            continue;
        }

        let tail_segs = turn.segments[split_at..].to_vec();
        let head_segs = turn.segments[..split_at].to_vec();
        let head_end = head_segs.iter().map(|s| seg_end(*s)).fold(0.0, f64::max);
        let tail_end = tail_segs.iter().map(|s| seg_end(*s)).fold(0.0, f64::max);
        let tail_start = tail_segs[0].start_time();
        let speaker = turn.speaker;
        out.push(Turn {
            start: turn.start,
            last_end: head_end,
            speaker,
            segments: head_segs,
        });
        remaining.push(Turn {
            start: tail_start,
            last_end: tail_end,
            speaker,
            segments: tail_segs,
        });
    }
    out
}

fn flush_paragraph(
    paragraphs: &mut Vec<Paragraph>,
    timestamp: &str,
    start_time: f64,
    speaker: Option<Speaker>,
    words: &mut Vec<String>,
) {
    paragraphs.push(Paragraph {
        timestamp: timestamp.to_string(),
        start_time,
        text: words.join(" "),
        speaker,
    });
    words.clear();
}

fn paragraphs_from_refs<S: SegmentLike>(
    segments: &[&S],
    pause_threshold: f64,
    break_on_speaker_change: bool,
) -> Vec<Paragraph> {
    if segments.is_empty() {
        return Vec::new();
    }

    let mut paragraphs: Vec<Paragraph> = Vec::new();
    let mut current_timestamp = format_timestamp(segments[0].start_time());
    let mut current_start = segments[0].start_time();
    let mut current_speaker: Option<Speaker> = segments[0].speaker();
    let mut current_words: Vec<String> = Vec::new();
    let mut current_chars: usize = 0;
    let mut sentence_count: usize = 0;
    let mut ends_sentence_flag = false;
    let mut last_end = segments[0].start_time();

    for seg in segments {
        let text = seg.text().trim();
        if text.is_empty() {
            continue;
        }
        let speaker = seg.speaker();

        if !current_words.is_empty() {
            let mut broke = false;
            if break_on_speaker_change && speaker != current_speaker {
                flush_paragraph(
                    &mut paragraphs,
                    &current_timestamp,
                    current_start,
                    current_speaker,
                    &mut current_words,
                );
                broke = true;
            } else {
                let gap = seg.start_time() - last_end;
                let break_at_sentence = ends_sentence_flag
                    && (gap >= pause_threshold
                        || sentence_count >= MAX_SENTENCES_PER_PARAGRAPH
                        || current_chars >= SOFT_MAX_CHARS);
                let break_hard = current_chars >= HARD_MAX_CHARS;

                if break_at_sentence || break_hard {
                    flush_paragraph(
                        &mut paragraphs,
                        &current_timestamp,
                        current_start,
                        current_speaker,
                        &mut current_words,
                    );
                    broke = true;
                }
            }

            if broke {
                current_timestamp = format_timestamp(seg.start_time());
                current_start = seg.start_time();
                current_speaker = speaker;
                current_chars = 0;
                sentence_count = 0;
            }
        } else {
            current_speaker = speaker;
        }

        current_words.push(text.to_string());
        current_chars += text.chars().count() + 1;
        sentence_count += count_sentence_ends(text);
        ends_sentence_flag = ends_sentence(text);
        last_end = last_end.max(seg_end(*seg));
    }

    if !current_words.is_empty() {
        paragraphs.push(Paragraph {
            timestamp: current_timestamp,
            start_time: current_start,
            text: current_words.join(" "),
            speaker: current_speaker,
        });
    }

    paragraphs
}

/// Group segments into flowing paragraphs with a leading timestamp.
///
/// Non-diarized streams keep storage order (legacy window-relative
/// timestamps must not be reordered) and break on pause, sentence count
/// and length. Diarized meetings cluster into per-speaker turns (emission
/// order within a lane), split each turn into readable paragraphs, then
/// order those paragraphs by start time.
pub fn group_into_paragraphs<S: SegmentLike>(
    segments: &[S],
    pause_threshold: f64,
) -> Vec<Paragraph> {
    if segments.is_empty() {
        return Vec::new();
    }

    let diarized = segments.iter().any(|s| s.speaker().is_some());
    if !diarized {
        let refs: Vec<&S> = segments.iter().collect();
        return paragraphs_from_refs(&refs, pause_threshold, false);
    }

    let turns = cluster_into_turns(segments, pause_threshold);
    let mut pieces: Vec<Paragraph> = Vec::new();
    for turn in &turns {
        pieces.extend(paragraphs_from_refs(&turn.segments, pause_threshold, false));
    }
    pieces.sort_by(|a, b| {
        a.start_time
            .partial_cmp(&b.start_time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct FixtureSegment {
        text: String,
        start_time: f64,
        end_time: f64,
        speaker: Option<Speaker>,
    }

    impl SegmentLike for FixtureSegment {
        fn text(&self) -> &str {
            &self.text
        }
        fn start_time(&self) -> f64 {
            self.start_time
        }
        fn end_time(&self) -> f64 {
            self.end_time
        }
        fn speaker(&self) -> Option<Speaker> {
            self.speaker
        }
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

    fn seg(text: &str, start: f64, end: f64, speaker: Speaker) -> FixtureSegment {
        FixtureSegment {
            text: text.to_string(),
            start_time: start,
            end_time: end,
            speaker: Some(speaker),
        }
    }

    fn texts(paragraphs: &[Paragraph]) -> Vec<(Option<Speaker>, &str)> {
        paragraphs
            .iter()
            .map(|p| (p.speaker, p.text.as_str()))
            .collect()
    }

    #[test]
    fn paragraph_grouping_matches_the_pinned_fixture() {
        let raw = include_str!("../tests/fixtures/paragraph_grouping.json");
        let fixture: Fixture = serde_json::from_str(raw).expect("valid fixture JSON");
        assert_eq!(fixture.pause_threshold, PAUSE_THRESHOLD_SECONDS);

        for case in &fixture.cases {
            let result = group_into_paragraphs(&case.segments, fixture.pause_threshold);

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

    // ── turn interruption (crosstalk vs. monologue) ─────────────────────

    #[test]
    fn interrupted_turn_closes_at_the_following_sentence_end() {
        // Me monologues without a pause; Them interjects mid-monologue. Me's
        // turn must not absorb everything: it closes at the first sentence
        // end after the interjection, so Them's turn sorts in between Me's
        // two turns instead of trailing behind the whole monologue.
        let segments = vec![
            seg("Let me explain the whole plan", 0.0, 0.5, Speaker::Me),
            seg("in detail because it's", 0.6, 1.1, Speaker::Me),
            seg("wait", 1.2, 1.7, Speaker::Them),
            seg("complicated.", 1.8, 2.3, Speaker::Me),
            seg("So let's start now", 3.0, 3.5, Speaker::Me),
        ];
        let result = group_into_paragraphs(&segments, PAUSE_THRESHOLD_SECONDS);
        assert_eq!(
            texts(&result),
            vec![
                (
                    Some(Speaker::Me),
                    "Let me explain the whole plan in detail because it's complicated."
                ),
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
            seg("First point.", 0.0, 0.5, Speaker::Me),
            seg("Second part continues", 0.6, 1.1, Speaker::Me),
            seg("quick question", 1.2, 1.7, Speaker::Them),
            seg("and concludes.", 1.8, 2.3, Speaker::Me),
            seg("New topic starts", 3.0, 3.5, Speaker::Me),
        ];
        let result = group_into_paragraphs(&segments, PAUSE_THRESHOLD_SECONDS);
        assert_eq!(
            texts(&result),
            vec![
                (
                    Some(Speaker::Me),
                    "First point. Second part continues and concludes."
                ),
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
            seg("so basically", 0.0, 0.5, Speaker::Me),
            seg("we were thinking", 0.6, 1.1, Speaker::Me),
            seg("right", 1.6, 2.1, Speaker::Them),
            seg("about moving the launch date", 2.2, 2.7, Speaker::Me),
            seg("to next quarter", 2.8, 3.3, Speaker::Me),
        ];
        let result = group_into_paragraphs(&segments, PAUSE_THRESHOLD_SECONDS);
        assert_eq!(
            texts(&result),
            vec![
                (Some(Speaker::Me), "so basically we were thinking"),
                (Some(Speaker::Them), "right"),
                (
                    Some(Speaker::Me),
                    "about moving the launch date to next quarter"
                ),
            ]
        );
    }

    #[test]
    fn same_speaker_keeps_emission_order_when_timestamps_jitter() {
        let segments = vec![
            seg("Je", 19.28, 19.38, Speaker::Them),
            seg("vous", 19.28, 19.38, Speaker::Them),
            seg("la", 19.30, 19.40, Speaker::Them),
            seg("mets", 19.29, 19.39, Speaker::Them),
            seg("dans", 19.32, 19.42, Speaker::Them),
            seg("le", 19.31, 19.41, Speaker::Them),
            seg("chat", 19.34, 19.44, Speaker::Them),
        ];
        let result = group_into_paragraphs(&segments, PAUSE_THRESHOLD_SECONDS);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "Je vous la mets dans le chat");
    }

    #[test]
    fn unpunctuated_overlap_closes_after_interrupt_hold() {
        let segments = vec![
            seg("aaaaaaaaaa", 0.0, 0.5, Speaker::Them),
            seg("bbbbbbbbbb", 0.4, 0.9, Speaker::Them),
            seg("interrupt", 0.5, 1.0, Speaker::Me),
            seg("cccccccccc", 0.8, 1.3, Speaker::Them),
            seg("dddddddddd", 1.6, 2.1, Speaker::Them),
        ];
        let result = group_into_paragraphs(&segments, PAUSE_THRESHOLD_SECONDS);
        assert_eq!(
            texts(&result),
            vec![
                (Some(Speaker::Them), "aaaaaaaaaa bbbbbbbbbb cccccccccc"),
                (Some(Speaker::Me), "interrupt"),
                (Some(Speaker::Them), "dddddddddd"),
            ]
        );
    }
}
