//! Transcript blocks for a completed meeting's post-meeting view. Paragraph
//! boundaries come from `souffle_schema::paragraphs::group_into_paragraphs`,
//! the same grouper the Markdown export, the summary turns and the MCP
//! sidecar use, so this screen and an export of the same meeting show the
//! same paragraphs. This module adds what is specific to the screen:
//! session-break rows between recording sessions, the `mm:ss` timestamp
//! format, and the per-word clickable breakdown (`TranscriptBlock.words`,
//! SOU-223) a `FlexboxLayout` in `transcript_section.slint` renders.

use souffle_lib::engine::{Speaker, TranscriptionSegment};
use souffle_lib::transcript::MeetingRecordingSession;
use souffle_schema::paragraphs::{PAUSE_THRESHOLD_SECONDS, group_into_paragraphs};

use crate::{SpeakerRole, TranscriptBlock, TranscriptWord};

/// `TranscriptBlock`'s `(has_speaker, speaker)` pair for a paragraph's lane,
/// shared by this post-meeting view and the live view (`live_transcript.rs`).
/// The display name is resolved in Slint (`SpeakerLabels.of`) so it follows
/// the UI locale; `None` (undiarized) carries no label, and its role is a
/// placeholder Slint never reads.
pub(crate) fn speaker_fields(speaker: Option<Speaker>) -> (bool, SpeakerRole) {
    match speaker {
        Some(Speaker::Me) => (true, SpeakerRole::Me),
        Some(Speaker::Them) => (true, SpeakerRole::Them),
        None => (false, SpeakerRole::Me),
    }
}

/// Port of `tokenizeTranscriptWords`/`isClickableTranscriptWord`
/// (`src/lib/utils/transcript-words.ts`) - milestone 8d. The Svelte
/// original uses a Unicode-property regex (`\p{L}\p{M}\p{N}'-`); this
/// treats `char::is_alphanumeric()` plus `'`/`-` as word characters, which
/// only differs for decomposed combining-mark sequences (`\p{M}` alone) -
/// not something ASR output produces (it's NFC-normalized), so not a real
/// gap in practice.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '\'' || c == '-'
}

/// At least 2 chars and contains a letter - excludes bare numbers and
/// single-character tokens (matches the Svelte reference exactly).
fn is_clickable_word(word: &str) -> bool {
    word.chars().count() >= 2 && word.chars().any(char::is_alphabetic)
}

/// Splits `text` into alternating runs of word-characters and everything
/// else (whitespace, punctuation), preserving order and never dropping
/// characters - `text` is exactly the concatenation of the returned pieces.
fn tokenize_words(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    let Some(&(_, first)) = chars.peek() else {
        return tokens;
    };
    let mut in_word = is_word_char(first);
    for (i, c) in chars {
        let word = is_word_char(c);
        if word != in_word {
            tokens.push(&text[start..i]);
            start = i;
            in_word = word;
        }
    }
    tokens.push(&text[start..]);
    tokens
}

/// Builds `TranscriptBlock.words` (see its doc comment). Each word-character
/// run owns the punctuation/whitespace after it so the separator cannot wrap
/// onto a new line by itself. The clickable `text` remains the exact source
/// word; only source-leading whitespace is omitted from display.
pub(crate) fn build_words(text: &str) -> Vec<TranscriptWord> {
    let mut words: Vec<TranscriptWord> = Vec::new();
    for token in tokenize_words(text) {
        if is_word_char(token.chars().next().unwrap_or(' ')) {
            words.push(TranscriptWord {
                clickable: is_clickable_word(token),
                text: token.into(),
                trailing_text: "".into(),
                provisional: false,
            });
        } else if let Some(previous) = words.last_mut() {
            previous.trailing_text = token.into();
        } else {
            let visible = token.trim_start_matches(char::is_whitespace);
            if !visible.is_empty() {
                words.push(TranscriptWord {
                    clickable: false,
                    text: visible.into(),
                    trailing_text: "".into(),
                    provisional: false,
                });
            }
        }
    }
    words
}

/// Wraps `build_words` in the `ModelRc` the generated `TranscriptBlock.words`
/// field needs - a Slint struct field of array type is always a model, never
/// a plain `Vec`, regardless of nesting.
fn transcript_words(text: &str) -> slint::ModelRc<TranscriptWord> {
    slint::ModelRc::new(slint::VecModel::from(build_words(text)))
}

/// Groups one contiguous run of segments (already known to belong to a
/// single recording session, or none) into paragraph blocks with the shared
/// grouper. The timestamp is re-formatted as `mm:ss` (the Slint display
/// format) rather than the paragraph's own `m:ss` export form.
fn paragraph_blocks(
    segments: &[TranscriptionSegment],
    session_index: Option<usize>,
) -> Vec<TranscriptBlock> {
    group_into_paragraphs(segments, PAUSE_THRESHOLD_SECONDS)
        .into_iter()
        .map(|p| {
            let (has_speaker, speaker) = speaker_fields(p.speaker);
            TranscriptBlock {
                is_session_break: false,
                has_speaker,
                speaker,
                timestamp: crate::timeline::format_duration(p.start_time).into(),
                words: transcript_words(&p.text),
                text: p.text.into(),
                recording_session_index: session_index.map(|i| i as i32).unwrap_or(-1),
                start_time: p.start_time as f32,
                end_label: "".into(),
                start_label: "".into(),
            }
        })
        .collect()
}

fn session_break_block() -> TranscriptBlock {
    TranscriptBlock {
        is_session_break: true,
        has_speaker: false,
        speaker: SpeakerRole::Me,
        timestamp: "".into(),
        text: "".into(),
        words: slint::ModelRc::new(slint::VecModel::from(Vec::<TranscriptWord>::new())),
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
    appended_any: &mut bool,
) {
    if end <= start {
        return;
    }
    if *appended_any {
        blocks.push(session_break_block());
    }
    blocks.extend(paragraph_blocks(&segments[start..end], session_index));
    *appended_any = true;
}

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
                &mut appended_any,
            );
        }
        push_range(
            &mut blocks,
            segments,
            start,
            end,
            Some(session_index),
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
            &mut appended_any,
        );
    }
    blocks
}

// SOU-187 milestone 8b (AC15): height estimate + windowing math for the
// virtualized transcript list. Slint has no built-in virtualization for
// variable-height rows - `i-slint-compiler-1.17.1/widgets/common/listview.slint`
// shows `std-widgets`' own `ListView`/`StandardListView` are a plain `for`
// loop inside a `ScrollView`/`Flickable`: every model entry is mounted
// unconditionally, regardless of what's on screen. This replaces "one row
// per model entry" with "spacer + only the on-screen slice", computed here
// since text wrapping/height can't be known from Slint's expression
// language ahead of actually laying a row out.

/// Rough text-wrap estimate, not real Slint text layout: assumes a fixed
/// character-per-line count for the transcript column's known width/font
/// size (13px body text in a ~370px-wide column, see
/// `components/transcript_section.slint`). A few px of error per row only
/// affects spacer sizing (scrollbar proportion), never the actually
/// mounted rows: those are still laid out for real inside a `VerticalLayout`.
const CHARS_PER_LINE: usize = 55;
const LINE_HEIGHT_PX: f32 = 18.0;
const HEADER_HEIGHT_PX: f32 = 20.0;
const BLOCK_SPACING_PX: f32 = 12.0; // matches the VerticalLayout's `spacing: 12px`
const SESSION_BREAK_HEIGHT_PX: f32 = 30.0;

fn estimate_block_height(block: &TranscriptBlock) -> f32 {
    let content_height = if block.is_session_break {
        SESSION_BREAK_HEIGHT_PX
    } else {
        let lines = (block.text.chars().count().max(1) as f32 / CHARS_PER_LINE as f32).ceil();
        HEADER_HEIGHT_PX + lines * LINE_HEIGHT_PX
    };
    content_height + BLOCK_SPACING_PX
}

/// Cumulative estimated y-offsets: `offsets[i]` is the top of `blocks[i]`,
/// and `offsets[blocks.len()]` is the total estimated content height. One
/// more entry than `blocks`, always non-decreasing.
pub fn compute_offsets(blocks: &[TranscriptBlock]) -> Vec<f32> {
    let mut offsets = Vec::with_capacity(blocks.len() + 1);
    let mut y = 0.0f32;
    offsets.push(y);
    for block in blocks {
        y += estimate_block_height(block);
        offsets.push(y);
    }
    offsets
}

pub struct VisibleWindow {
    pub start: usize,
    pub end: usize, // exclusive
    pub spacer_before: f32,
    pub spacer_after: f32,
}

/// Which block indices (by index into the `blocks` slice `offsets` was
/// built from) fall within `[scroll_top, scroll_top + viewport_height]`
/// plus `margin` on each side, given `offsets` from `compute_offsets`.
/// Always returns at least one block (when there is at least one) so the
/// caller never has to special-case an empty slice.
pub fn visible_window(
    offsets: &[f32],
    scroll_top: f32,
    viewport_height: f32,
    margin: f32,
) -> VisibleWindow {
    let block_count = offsets.len().saturating_sub(1);
    if block_count == 0 {
        return VisibleWindow {
            start: 0,
            end: 0,
            spacer_before: 0.0,
            spacer_after: 0.0,
        };
    }
    let total = offsets[block_count];
    let lo = (scroll_top - margin).max(0.0);
    let hi = (scroll_top + viewport_height + margin).min(total);

    // First block whose start offset is >= lo, then step back one: that
    // earlier block may still overlap [lo, hi) even though it starts
    // before lo.
    let first_at_or_after_lo = offsets[..block_count].partition_point(|&top| top < lo);
    let start = first_at_or_after_lo.saturating_sub(1).min(block_count - 1);

    // First block whose start offset is > hi is the exclusive end.
    let end = offsets[..=block_count]
        .partition_point(|&top| top <= hi)
        .clamp(start + 1, block_count);

    VisibleWindow {
        start,
        end,
        spacer_before: offsets[start],
        spacer_after: total - offsets[end],
    }
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
        assert_eq!(
            (blocks[0].has_speaker, blocks[0].speaker),
            (true, SpeakerRole::Me)
        );
        assert_eq!(
            (blocks[2].has_speaker, blocks[2].speaker),
            (true, SpeakerRole::Them)
        );
    }

    /// The post-meeting view runs the same grouper as the export, proven on
    /// the shared fixture's diarized cases: block texts and speaker labels
    /// must match the pinned paragraphs (through `TranscriptionSegment`'s
    /// `SegmentLike` impl and this module's label mapping).
    #[test]
    fn diarized_blocks_match_the_shared_paragraph_fixture() {
        use serde_json::Value;

        fn speaker_of(value: &Value) -> Option<Speaker> {
            value["speaker"].as_str().and_then(Speaker::parse)
        }

        let raw = include_str!("../../souffle-schema/tests/fixtures/paragraph_grouping.json");
        let fixture: Value = serde_json::from_str(raw).expect("valid fixture JSON");
        let cases = fixture["cases"].as_array().expect("cases array");
        let wanted = [
            "diarized_crosstalk_word_level",
            "diarized_monologue_interjection",
        ];
        for name in wanted {
            let case = cases
                .iter()
                .find(|c| c["name"] == name)
                .unwrap_or_else(|| panic!("fixture case '{name}' missing"));
            let segments: Vec<TranscriptionSegment> = case["segments"]
                .as_array()
                .expect("segments array")
                .iter()
                .map(|s| {
                    seg(
                        s["text"].as_str().expect("text"),
                        s["start_time"].as_f64().expect("start_time"),
                        s["end_time"].as_f64().expect("end_time"),
                        speaker_of(s),
                    )
                })
                .collect();

            let blocks = build_transcript_blocks(&segments, &[]);
            let actual: Vec<((bool, SpeakerRole), String)> = blocks
                .iter()
                .map(|b| ((b.has_speaker, b.speaker), b.text.to_string()))
                .collect();
            let expected: Vec<((bool, SpeakerRole), String)> = case["expected"]
                .as_array()
                .expect("expected array")
                .iter()
                .map(|p| {
                    (
                        speaker_fields(speaker_of(p)),
                        p["text"].as_str().expect("text").to_string(),
                    )
                })
                .collect();
            assert_eq!(actual, expected, "case '{name}'");
        }
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

    fn plain_block(text: &str) -> TranscriptBlock {
        TranscriptBlock {
            is_session_break: false,
            has_speaker: true,
            speaker: SpeakerRole::Me,
            timestamp: "00:00".into(),
            words: transcript_words(text),
            text: text.into(),
            recording_session_index: -1,
            start_time: 0.0,
            end_label: "".into(),
            start_label: "".into(),
        }
    }

    #[test]
    fn compute_offsets_is_monotonic_and_matches_block_count() {
        let blocks: Vec<_> = (0..10).map(|i| plain_block(&format!("bloc {i}"))).collect();
        let offsets = compute_offsets(&blocks);
        assert_eq!(offsets.len(), blocks.len() + 1);
        assert_eq!(offsets[0], 0.0);
        assert!(
            offsets.windows(2).all(|w| w[1] > w[0]),
            "strictly increasing"
        );
    }

    #[test]
    fn visible_window_returns_everything_when_it_all_fits() {
        let blocks: Vec<_> = (0..5).map(|i| plain_block(&format!("bloc {i}"))).collect();
        let offsets = compute_offsets(&blocks);
        let window = visible_window(&offsets, 0.0, 10_000.0, 0.0);
        assert_eq!((window.start, window.end), (0, 5));
        assert_eq!(window.spacer_before, 0.0);
        assert_eq!(window.spacer_after, 0.0);
    }

    /// AC15's actual proof: scrolled to the middle of a 2000-paragraph
    /// transcript, only a small, bounded slice is ever "mounted" (returned
    /// as the visible range), independent of the 2000 total - this is the
    /// same predicate `main.rs` uses to build the Slint model, so this
    /// slice size is the real active-node count, not a separate estimate.
    #[test]
    fn visible_window_slices_a_huge_transcript_to_a_bounded_count() {
        let blocks: Vec<_> = (0..2000)
            .map(|i| {
                plain_block(&format!(
                    "Paragraphe numéro {i} avec un peu de texte réaliste."
                ))
            })
            .collect();
        let offsets = compute_offsets(&blocks);
        let total_height = *offsets.last().unwrap();
        let viewport_height = 260.0;
        let margin = 3.0 * viewport_height;

        let window = visible_window(&offsets, total_height / 2.0, viewport_height, margin);

        assert!(window.start > 0, "not scrolled to the very top");
        assert!(window.end < blocks.len(), "not scrolled to the very bottom");
        let mounted = window.end - window.start;
        assert!(
            mounted < 60,
            "expected a small bounded window regardless of 2000 total blocks, got {mounted}"
        );
        assert!(window.spacer_before > 0.0);
        assert!(window.spacer_after > 0.0);
    }

    #[test]
    fn visible_window_at_the_very_end_still_bounded_and_has_no_after_spacer() {
        let blocks: Vec<_> = (0..2000)
            .map(|i| plain_block(&format!("bloc {i}")))
            .collect();
        let offsets = compute_offsets(&blocks);
        let total_height = *offsets.last().unwrap();
        let viewport_height = 260.0;
        let margin = 3.0 * viewport_height;

        let window = visible_window(&offsets, total_height, viewport_height, margin);

        assert_eq!(window.end, blocks.len());
        assert_eq!(window.spacer_after, 0.0);
        assert!(window.end - window.start < 60);
    }

    #[test]
    fn tokenize_words_never_loses_characters() {
        let text = "Bonjour, comment ça va - super bien !";
        let tokens = tokenize_words(text);
        assert_eq!(tokens.concat(), text);
    }

    #[test]
    fn is_clickable_word_excludes_short_and_numeric_tokens() {
        assert!(is_clickable_word("bonjour"));
        assert!(is_clickable_word("ça"));
        assert!(!is_clickable_word("a"));
        assert!(!is_clickable_word("42"));
        assert!(!is_clickable_word(""));
    }

    fn word(text: &str, trailing_text: &str, clickable: bool) -> TranscriptWord {
        TranscriptWord {
            text: text.into(),
            trailing_text: trailing_text.into(),
            clickable,
            provisional: false,
        }
    }

    #[test]
    fn build_words_marks_clickable_words_only() {
        // "42" is numeric-only (not clickable), "ans" is a real word.
        let words = build_words("Bonjour, 42 ans.");
        assert_eq!(
            words,
            vec![
                word("Bonjour", ", ", true),
                word("42", " ", false),
                word("ans", ".", true),
            ]
        );
    }

    #[test]
    fn build_words_never_alters_token_text() {
        // No Markdown escaping needed anymore - a word's text round-trips
        // through `build_words` unchanged, including characters that used
        // to need escaping in the old Markdown-link render.
        let words = build_words("valeur * 2 <ok>");
        assert_eq!(
            words,
            vec![
                word("valeur", " * ", true),
                // "2" is its own word-char run (digits count as word chars
                // for tokenization, just not for clickability).
                word("2", " <", false),
                word("ok", ">", true),
            ]
        );
    }

    #[test]
    fn build_words_round_trips_through_the_real_click_target() {
        // What `TouchArea.clicked` hands back to Rust (`word.text`) must
        // match the original word exactly, apostrophe included.
        let words = build_words("aujourd'hui");
        assert_eq!(words, vec![word("aujourd'hui", "", true)]);
    }

    /// SOU-223 "Couvrir les occurrences répétées": `build_words` decides per
    /// token via `is_clickable_word`, with no "already saw this word" state,
    /// so every repeated clickable occurrence is independently marked, and a
    /// repeated non-admissible token (here, a bare digit) stays non-clickable
    /// every time.
    #[test]
    fn build_words_marks_every_repeated_occurrence() {
        let words = build_words("3 3 bien bien.");
        assert_eq!(
            words,
            vec![
                word("3", " ", false),
                word("3", " ", false),
                word("bien", " ", true),
                word("bien", ".", true),
            ]
        );
    }

    #[test]
    fn build_words_concatenation_never_loses_non_leading_characters() {
        let text = "Bonjour, comment \u{e7}a va - super bien !";
        let words = build_words(text);
        let rebuilt: String = words
            .iter()
            .flat_map(|word| [word.text.as_str(), word.trailing_text.as_str()])
            .collect();
        assert_eq!(rebuilt, text);
    }

    #[test]
    fn build_words_never_creates_a_leading_whitespace_item() {
        let words = build_words("  Bonjour, données utiles");
        assert!(
            words
                .iter()
                .all(|word| !word.text.starts_with(char::is_whitespace)),
            "a flex item still starts with whitespace: {words:?}"
        );
        assert_eq!(
            words
                .iter()
                .flat_map(|word| [word.text.as_str(), word.trailing_text.as_str()])
                .collect::<String>(),
            "Bonjour, données utiles"
        );
    }
}
