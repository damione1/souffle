//! Live transcript state for a meeting in progress.
//!
//! Live and saved transcripts share the canonical paragraph grouper. Only
//! a bounded window is regrouped, so delayed lanes can complete an interruption
//! without a model-specific latency horizon or rescanning an entire meeting.
//! Previews retain their audio timestamps and join that pass alongside finals.

use souffle_lib::engine::{Speaker, TranscriptionSegment};
use souffle_schema::paragraphs::{PAUSE_THRESHOLD_SECONDS, group_into_paragraph_segments};

use crate::transcript::{build_words, speaker_fields};
use crate::{TranscriptBlock, TranscriptWord, timeline};

/// `TranscriptBlock.words` for a live paragraph (SOU-256 AC5): finalized
/// text tokenized like the post-meeting transcript, so its words open the
/// dictionary-alias popover when clicked, followed by the provisional
/// (tentative) tail with every token non-clickable - that text is still
/// being rewritten by the engine and is not worth a dictionary entry yet -
/// and marked provisional, so it is drawn dimmed instead of like final text.
pub(crate) fn provisional_words(finalized: &str, provisional: Option<&str>) -> Vec<TranscriptWord> {
    let mut words = build_words(finalized);
    if let Some(tail) = provisional.filter(|t| !t.is_empty()) {
        if let Some(previous) = words.last_mut() {
            let mut trailing = previous.trailing_text.to_string();
            trailing.push(' ');
            previous.trailing_text = trailing.into();
        }
        words.extend(build_words(tail).into_iter().map(|w| TranscriptWord {
            clickable: false,
            provisional: true,
            ..w
        }));
    }
    words
}

fn live_words(finalized: &str, provisional: Option<&str>) -> slint::ModelRc<TranscriptWord> {
    slint::ModelRc::new(slint::VecModel::from(provisional_words(
        finalized,
        provisional,
    )))
}

/// Paragraphs shown in the live view. One preceding paragraph is retained
/// as grouping context when the oldest visible paragraph follows a handoff.
const LIVE_PARAGRAPH_WINDOW: usize = 30;

/// One paragraph produced by the shared grouping policy.
struct LivePara {
    speaker: Option<Speaker>,
    /// The first segment's audio timestamp, never wall-clock arrival time.
    timestamp: String,
    start_time: f64,
    /// Committed words, space-joined.
    text: String,
}

impl LivePara {
    fn from_segments(segments: &[&TranscriptionSegment]) -> Self {
        let first = segments[0];
        Self {
            speaker: first.speaker,
            timestamp: timeline::format_duration(first.start_time),
            start_time: first.start_time,
            text: segments
                .iter()
                .filter(|s| s.is_final)
                .map(|s| s.text.trim())
                .collect::<Vec<_>>()
                .join(" "),
        }
    }

    fn to_slint_block(&self, tentative_suffix: Option<&str>) -> TranscriptBlock {
        let text = match tentative_suffix {
            Some(t) if !t.is_empty() => {
                if self.text.is_empty() {
                    t.to_owned()
                } else {
                    format!("{} {}", self.text, t)
                }
            }
            _ => self.text.clone(),
        };
        let (has_speaker, speaker) = speaker_fields(self.speaker);
        TranscriptBlock {
            is_session_break: false,
            has_speaker,
            speaker,
            timestamp: self.timestamp.clone().into(),
            text: text.into(),
            words: live_words(&self.text, tentative_suffix),
            recording_session_index: -1,
            start_time: self.start_time as f32,
            end_label: "".into(),
            start_label: "".into(),
        }
    }
}

/// One revisable word or phrase per source, with its audio position. It
/// stays visible until that lane publishes a revision, final or explicit
/// withdrawal; wall-clock expiry previously made pending words disappear.
#[derive(Default)]
pub struct TentativeSlot {
    segment: Option<TranscriptionSegment>,
}

impl TentativeSlot {
    fn set(&mut self, segment: &TranscriptionSegment) {
        self.segment = (!segment.text.trim().is_empty()).then(|| segment.clone());
    }

    fn clear(&mut self) {
        self.segment = None;
    }

    fn active_text(&self) -> Option<&str> {
        self.segment.as_ref().map(|s| s.text.trim())
    }
}

/// The live transcript state machine. All mutations happen on the Slint
/// main thread (called from `invoke_from_event_loop`), so no locking needed.
pub struct LiveTranscript {
    generation: u64,
    /// Finals in emission order, bounded by the visible paragraph window.
    /// Text is immutable; grouping can still change for a delayed other lane.
    pub(crate) finals: Vec<TranscriptionSegment>,
    /// Pending-word slot for Me lane.
    pub tentative_me: TentativeSlot,
    /// Pending-word slot for Them lane.
    pub tentative_them: TentativeSlot,
    /// Pending-word slot for undiarised / dictation lane.
    pub tentative_none: TentativeSlot,
}

impl LiveTranscript {
    pub fn new() -> Self {
        Self {
            generation: 0,
            finals: Vec::new(),
            tentative_me: TentativeSlot::default(),
            tentative_them: TentativeSlot::default(),
            tentative_none: TentativeSlot::default(),
        }
    }

    /// Push a final segment. Clears only the tentative slot of the
    /// finalizing speaker (SOU-061 invariant).
    pub fn push_final(&mut self, seg: &TranscriptionSegment) {
        // Clear the matching tentative slot.
        match seg.speaker {
            Some(Speaker::Me) => self.tentative_me.clear(),
            Some(Speaker::Them) => self.tentative_them.clear(),
            None => self.tentative_none.clear(),
        }

        if seg.text.trim().is_empty() {
            return;
        }
        self.finals.push(seg.clone());
        self.trim_history();
    }

    /// Push a tentative (non-final) word for a speaker lane.
    pub fn push_tentative(&mut self, seg: &TranscriptionSegment) {
        match seg.speaker {
            Some(Speaker::Me) => self.tentative_me.set(seg),
            Some(Speaker::Them) => self.tentative_them.set(seg),
            None => self.tentative_none.set(seg),
        }
    }

    fn trim_history(&mut self) {
        let groups = group_into_paragraph_segments(&self.finals, PAUSE_THRESHOLD_SECONDS);
        let count = groups.len().saturating_sub(LIVE_PARAGRAPH_WINDOW + 1);
        if count == 0 {
            return;
        }
        let consumed: std::collections::HashSet<*const TranscriptionSegment> = groups[..count]
            .iter()
            .flatten()
            .map(|s| std::ptr::from_ref(*s))
            .collect();
        // Compute membership before retain moves any elements.
        let keep: Vec<_> = self
            .finals
            .iter()
            .map(|s| !consumed.contains(&std::ptr::from_ref(s)))
            .collect();
        let mut keep = keep.into_iter();
        self.finals.retain(|_| keep.next().unwrap_or(false));
    }

    /// Render timestamped previews through the same grouper as finals.
    /// A preview can open a new turn; it never mutates finalized text.
    /// The visible window can be recut by a delayed lane or a revision.
    pub fn build_blocks(&self) -> Vec<TranscriptBlock> {
        let mut segments = self.finals.clone();
        segments.extend(
            [
                &self.tentative_me,
                &self.tentative_them,
                &self.tentative_none,
            ]
            .into_iter()
            .filter_map(|slot| slot.segment.clone()),
        );
        let mut blocks = Vec::new();
        for group in group_into_paragraph_segments(&segments, PAUSE_THRESHOLD_SECONDS) {
            let provisional = group
                .iter()
                .filter(|s| !s.is_final)
                .map(|s| s.text.trim())
                .collect::<Vec<_>>()
                .join(" ");
            blocks.push(LivePara::from_segments(&group).to_slint_block(Some(&provisional)));
        }
        let overflow = blocks.len().saturating_sub(LIVE_PARAGRAPH_WINDOW);
        blocks.drain(..overflow);
        blocks
    }

    /// True if there is nothing to show (no blocks, no tentative).
    pub fn is_empty(&self) -> bool {
        self.finals.is_empty()
            && self.tentative_me.active_text().is_none()
            && self.tentative_them.active_text().is_none()
            && self.tentative_none.active_text().is_none()
    }

    /// Reset everything (called by `clear_live_transcript`).
    pub fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.finals.clear();
        self.tentative_me.clear();
        self.tentative_them.clear();
        self.tentative_none.clear();
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::SpeakerRole;

    #[test]
    fn resumed_preview_opens_a_timestamped_turn_after_the_other_speaker() {
        let mut live = LiveTranscript::new();
        live.push_final(&seg("My first turn.", 0.0, 1.0, true, Some(Speaker::Me)));
        live.push_final(&seg("Your answer.", 2.0, 3.0, true, Some(Speaker::Them)));
        live.push_tentative(&seg("My next turn", 5.0, 6.0, false, Some(Speaker::Me)));
        let blocks = live.build_blocks();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].text.as_str(), "My first turn.");
        assert_eq!(blocks[2].timestamp.as_str(), "0:05");
        assert_eq!(blocks[2].start_time, 5.0);
        live.push_final(&seg("My next turn.", 5.0, 6.0, true, Some(Speaker::Me)));
        assert_eq!(live.build_blocks().len(), 3);
    }

    #[test]
    fn interrupted_monologue_matches_the_saved_dialogue() {
        let segments = vec![
            seg("Let me explain", 0.0, 0.5, true, Some(Speaker::Me)),
            seg("in detail because it is", 0.6, 1.1, true, Some(Speaker::Me)),
            seg("wait", 1.2, 1.7, true, Some(Speaker::Them)),
            seg("complicated.", 1.8, 2.3, true, Some(Speaker::Me)),
            seg("So let us start", 3.0, 3.5, true, Some(Speaker::Me)),
        ];
        let mut live = LiveTranscript::new();
        for segment in &segments {
            live.push_final(segment);
        }
        let saved =
            souffle_schema::paragraphs::group_into_paragraphs(&segments, PAUSE_THRESHOLD_SECONDS);
        let blocks = live.build_blocks();
        assert_eq!(blocks.len(), 3);
        assert_eq!(
            blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(),
            saved.iter().map(|p| p.text.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn live_monologue_breaks_after_four_sentences() {
        let mut live = LiveTranscript::new();
        for i in 0..9 {
            live.push_final(&seg(
                "One sentence.",
                f64::from(i),
                f64::from(i) + 0.8,
                true,
                Some(Speaker::Me),
            ));
        }
        assert_eq!(live.build_blocks().len(), 3);
    }

    #[test]
    fn first_mono_tentative_has_a_block_without_finals() {
        use slint::Model;
        let mut live = LiveTranscript::new();
        live.push_tentative(&TranscriptionSegment {
            text: "Premier aperçu".into(),
            start_time: 0.0,
            end_time: 1.5,
            is_final: false,
            language: None,
            confidence: None,
            speaker: None,
        });
        let blocks = live.build_blocks();
        assert_eq!(blocks.len(), 1);
        assert!(!blocks[0].has_speaker);
        assert_eq!(blocks[0].text.as_str(), "Premier aperçu");
        assert!(
            blocks[0]
                .words
                .iter()
                .all(|w| w.provisional && !w.clickable)
        );
        assert!(live.finals.is_empty());
    }

    fn seg(
        text: &str,
        start: f64,
        end: f64,
        is_final: bool,
        speaker: Option<Speaker>,
    ) -> TranscriptionSegment {
        TranscriptionSegment {
            text: text.into(),
            start_time: start,
            end_time: end,
            is_final,
            language: None,
            confidence: None,
            speaker,
        }
    }

    // AC1: live grouper produces same blocks as build_transcript_blocks on the same finals.
    #[test]
    fn live_grouper_matches_batch_on_same_speaker_change() {
        let mut lt = LiveTranscript::new();
        lt.push_final(&seg("Bonjour", 0.0, 1.0, true, Some(Speaker::Me)));
        lt.push_final(&seg("monde", 1.2, 2.0, true, Some(Speaker::Me)));
        lt.push_final(&seg("Salut", 2.5, 3.0, true, Some(Speaker::Them)));
        let blocks = lt.build_blocks();
        // Me paragraph, Them paragraph
        assert_eq!(blocks.len(), 2);
        assert_eq!(
            (blocks[0].has_speaker, blocks[0].speaker),
            (true, SpeakerRole::Me)
        );
        assert_eq!(blocks[0].text.as_str(), "Bonjour monde");
        assert_eq!(
            (blocks[1].has_speaker, blocks[1].speaker),
            (true, SpeakerRole::Them)
        );
        assert_eq!(blocks[1].text.as_str(), "Salut");
    }

    // AC1b: pause split same speaker.
    #[test]
    fn live_grouper_splits_on_pause() {
        let mut lt = LiveTranscript::new();
        lt.push_final(&seg("Un", 0.0, 1.0, true, Some(Speaker::Me)));
        // pause > 1.5s → new paragraph
        lt.push_final(&seg("Deux", 5.0, 6.0, true, Some(Speaker::Me)));
        let blocks = lt.build_blocks();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].text.as_str(), "Un");
        assert_eq!(blocks[1].text.as_str(), "Deux");
    }

    // AC2: a final on Me does not clear Them's tentative (SOU-061).
    #[test]
    fn final_on_me_does_not_clear_them_tentative() {
        let mut lt = LiveTranscript::new();
        lt.push_tentative(&seg("bonjour", 0.0, 0.5, false, Some(Speaker::Them)));
        lt.push_final(&seg("Hello", 1.0, 1.5, true, Some(Speaker::Me)));
        // Them's tentative must still be active.
        assert_eq!(lt.tentative_them.active_text(), Some("bonjour"));
    }

    // AC2b: a tentative on Me does not affect Them's tentative.
    #[test]
    fn tentative_on_me_does_not_clear_them_tentative() {
        let mut lt = LiveTranscript::new();
        lt.push_tentative(&seg("hola", 0.0, 0.3, false, Some(Speaker::Them)));
        lt.push_tentative(&seg("hello", 0.1, 0.4, false, Some(Speaker::Me)));
        assert_eq!(lt.tentative_them.active_text(), Some("hola"));
        assert_eq!(lt.tentative_me.active_text(), Some("hello"));
    }

    // A pending word stays visible, dimmed, until the engine confirms it:
    // it used to expire after 5 s and come back at the end of the meeting.
    #[test]
    fn a_pending_word_stays_until_its_final_and_is_drawn_provisional() {
        use slint::Model;
        let mut lt = LiveTranscript::new();
        lt.push_final(&seg("yellow", 17.0, 17.4, true, Some(Speaker::Them)));
        lt.push_tentative(&seg("lemons", 17.4, 17.4, false, Some(Speaker::Them)));
        assert_eq!(lt.tentative_them.active_text(), Some("lemons"));

        let block = &lt.build_blocks()[0];
        let words: Vec<(String, String, bool)> = block
            .words
            .iter()
            .map(|w| {
                (
                    w.text.to_string(),
                    w.trailing_text.to_string(),
                    w.provisional,
                )
            })
            .collect();
        assert_eq!(
            words,
            vec![
                ("yellow".to_string(), " ".to_string(), false),
                ("lemons".to_string(), "".to_string(), true),
            ]
        );

        lt.push_final(&seg("lemons.", 17.4, 17.9, true, Some(Speaker::Them)));
        assert_eq!(lt.tentative_them.active_text(), None);
        assert!(lt.build_blocks()[0].words.iter().all(|w| !w.provisional));
    }

    // AC4: timestamp is fixed at paragraph creation.
    #[test]
    fn timestamp_fixed_at_paragraph_creation() {
        let mut lt = LiveTranscript::new();
        lt.push_final(&seg("Premier mot", 65.0, 66.0, true, Some(Speaker::Me)));
        let ts1 = lt.build_blocks()[0].timestamp.clone();
        lt.push_final(&seg("deuxième", 66.5, 67.0, true, Some(Speaker::Me)));
        let ts2 = lt.build_blocks()[0].timestamp.clone();
        // The block's timestamp must not change as new words are appended.
        assert_eq!(ts1, ts2);
        // And it must match format_duration(65.0) = "1:05".
        assert_eq!(ts1.as_str(), "1:05");
    }

    // Final text remains immutable when a later turn arrives.
    #[test]
    fn later_turns_do_not_mutate_final_text() {
        let mut lt = LiveTranscript::new();
        lt.push_final(&seg("Alpha", 0.0, 1.0, true, Some(Speaker::Me)));
        lt.push_final(&seg("Beta", 20.0, 21.0, true, Some(Speaker::Me)));
        assert_eq!(lt.build_blocks()[0].text.as_str(), "Alpha");
    }

    // SOU-256 (reported live by Damien): a short Me interjection during a
    // long Them monologue showed under it, then jumped above it once Me's
    // paragraph was committed first - committed paragraphs were listed
    // before tail ones regardless of time. Paragraphs are ordered by start
    // time, like the post-meeting grouper. Continuing after the interjection
    // now opens a third turn rather than extending the initial monologue.
    #[test]
    fn continued_monologue_stays_after_the_interjection() {
        let mut lt = LiveTranscript::new();
        let order = |lt: &LiveTranscript| -> Vec<SpeakerRole> {
            lt.build_blocks().iter().map(|b| b.speaker).collect()
        };
        // Them talks without a pause from 0:05 to 0:40. Me's interjection
        // at 0:17 must stay between the two parts of Them's monologue.
        let mut me_spoke = false;
        for i in 0..30 {
            let t = 5.0 + f64::from(i) * 1.2;
            lt.push_final(&seg("et encore", t, t + 1.0, true, Some(Speaker::Them)));
            if t >= 17.0 && !me_spoke {
                lt.push_final(&seg("Un deux trois", 17.0, 18.0, true, Some(Speaker::Me)));
                assert_eq!(
                    order(&lt),
                    vec![SpeakerRole::Them, SpeakerRole::Me, SpeakerRole::Them]
                );
                me_spoke = true;
            }
        }
        assert_eq!(
            order(&lt),
            vec![SpeakerRole::Them, SpeakerRole::Me, SpeakerRole::Them]
        );
    }

    #[test]
    fn delayed_lane_and_preview_revisions_keep_the_canonical_turns() {
        use slint::Model;
        let mut live = LiveTranscript::new();
        // Batch decoding can deliver Me's next window before Them's answer.
        let finals = [
            seg("The proposal.", 10.0, 11.0, true, Some(Speaker::Me)),
            seg("Let us continue.", 12.0, 13.0, true, Some(Speaker::Me)),
            seg("Wait.", 11.4, 11.8, true, Some(Speaker::Them)),
        ];
        for segment in &finals {
            live.push_final(segment);
        }
        live.push_tentative(&seg(
            "Earlier reply",
            14.0,
            14.5,
            false,
            Some(Speaker::Them),
        ));
        live.push_tentative(&seg("Next reply", 17.0, 18.0, false, Some(Speaker::Me)));
        live.push_tentative(&seg(
            "Revised reply",
            14.0,
            15.0,
            false,
            Some(Speaker::Them),
        ));
        let blocks = live.build_blocks();
        assert_eq!(blocks.len(), 5);
        assert_eq!(blocks[3].text.as_str(), "Revised reply");
        assert_eq!(blocks[3].timestamp.as_str(), "0:14");
        assert_eq!(blocks[4].text.as_str(), "Next reply");
        assert!(
            blocks[4]
                .words
                .iter()
                .all(|w| w.provisional && !w.clickable)
        );
        // Retraction removes only Them's preview and leaves every final intact.
        live.push_tentative(&seg("", 14.0, 15.0, false, Some(Speaker::Them)));
        let blocks = live.build_blocks();
        assert_eq!(blocks.len(), 4);
        let saved =
            souffle_schema::paragraphs::group_into_paragraphs(&finals, PAUSE_THRESHOLD_SECONDS);
        assert_eq!(
            blocks[..3]
                .iter()
                .map(|b| b.text.as_str())
                .collect::<Vec<_>>(),
            saved.iter().map(|p| p.text.as_str()).collect::<Vec<_>>()
        );
        assert_eq!(blocks[3].text.as_str(), "Next reply");
    }

    #[test]
    fn preview_only_turns_sort_by_audio_time_and_never_default_to_zero() {
        let mut live = LiveTranscript::new();
        live.push_tentative(&seg("Me", 30.0, 31.0, false, Some(Speaker::Me)));
        live.push_tentative(&seg("Them", 25.0, 26.0, false, Some(Speaker::Them)));
        let blocks = live.build_blocks();
        assert_eq!(blocks[0].speaker, SpeakerRole::Them);
        assert_eq!(blocks[0].timestamp.as_str(), "0:25");
        assert_eq!(blocks[1].timestamp.as_str(), "0:30");
    }

    #[test]
    fn bounded_tail_keeps_long_dialogue_equal_to_saved_window() {
        let mut live = LiveTranscript::new();
        let mut finals = Vec::new();
        for i in 0..300 {
            let speaker = if i % 2 == 0 {
                Speaker::Me
            } else {
                Speaker::Them
            };
            let start = f64::from(i) * 2.0;
            let segment = seg(
                &format!("Turn {i}."),
                start,
                start + 0.5,
                true,
                Some(speaker),
            );
            live.push_final(&segment);
            finals.push(segment);
            assert!(live.finals.len() <= LIVE_PARAGRAPH_WINDOW + 1);
            let saved =
                souffle_schema::paragraphs::group_into_paragraphs(&finals, PAUSE_THRESHOLD_SECONDS);
            let skip = saved.len().saturating_sub(LIVE_PARAGRAPH_WINDOW);
            let blocks = live.build_blocks();
            assert_eq!(
                blocks
                    .iter()
                    .map(|b| (b.timestamp.as_str(), b.text.as_str()))
                    .collect::<Vec<_>>(),
                saved[skip..]
                    .iter()
                    .map(|p| (p.timestamp.as_str(), p.text.as_str()))
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn a_late_lane_can_split_visible_history_beyond_eight_seconds() {
        let mut live = LiveTranscript::new();
        let mut finals = Vec::new();
        for i in 0..40 {
            let t = f64::from(i) * 3.0;
            let speaker = if i % 2 == 0 {
                Speaker::Them
            } else {
                Speaker::Me
            };
            let s = seg(
                &format!("Earlier turn {i}."),
                t,
                t + 0.5,
                true,
                Some(speaker),
            );
            live.push_final(&s);
            finals.push(s);
        }
        for s in [
            seg("The long opening.", 120.0, 127.0, true, Some(Speaker::Me)),
            seg("The continuation.", 127.0, 134.0, true, Some(Speaker::Me)),
            seg("After a pause.", 138.0, 139.0, true, Some(Speaker::Me)),
            seg(
                "A delayed interruption.",
                123.0,
                124.0,
                true,
                Some(Speaker::Them),
            ),
        ] {
            live.push_final(&s);
            finals.push(s);
        }
        let blocks = live.build_blocks();
        let saved =
            souffle_schema::paragraphs::group_into_paragraphs(&finals, PAUSE_THRESHOLD_SECONDS);
        let last = &saved[saved.len() - LIVE_PARAGRAPH_WINDOW..];
        assert_eq!(
            blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(),
            last.iter().map(|p| p.text.as_str()).collect::<Vec<_>>()
        );
        assert_eq!(blocks[27].text.as_str(), "A delayed interruption.");
    }

    #[test]
    fn long_jittered_monologue_keeps_sentence_boundaries_after_eviction() {
        let mut live = LiveTranscript::new();
        let mut finals = Vec::new();
        for i in 0..160 {
            let t = f64::from(i);
            for s in [
                seg(
                    &format!("Sentence{i}"),
                    t + 0.1,
                    t + 0.5,
                    true,
                    Some(Speaker::Me),
                ),
                seg("ends.", t, t + 0.8, true, Some(Speaker::Me)),
            ] {
                live.push_final(&s);
                finals.push(s);
            }
        }
        let before = live.build_blocks();
        let retained = live.finals.len();
        live.push_tentative(&seg(
            "Speculative interruption",
            151.0,
            152.0,
            false,
            Some(Speaker::Them),
        ));
        live.build_blocks();
        live.push_tentative(&seg("", 151.0, 152.0, false, Some(Speaker::Them)));
        assert_eq!(live.finals.len(), retained);
        let saved =
            souffle_schema::paragraphs::group_into_paragraphs(&finals, PAUSE_THRESHOLD_SECONDS);
        let last = &saved[saved.len() - LIVE_PARAGRAPH_WINDOW..];
        assert_eq!(
            before.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(),
            last.iter().map(|p| p.text.as_str()).collect::<Vec<_>>()
        );
        assert_eq!(
            live.build_blocks()
                .iter()
                .map(|b| b.text.to_string())
                .collect::<Vec<_>>(),
            before
                .iter()
                .map(|b| b.text.to_string())
                .collect::<Vec<_>>()
        );
    }

    // SOU-256: the very first partial of a meeting, before any final,
    // must show - it used to be dropped by an early return on an empty
    // paragraph window, leaving a blank view with the placeholder hidden.
    #[test]
    fn first_partial_shows_before_any_final() {
        let mut lt = LiveTranscript::new();
        lt.push_tentative(&seg("Bonjour", 0.0, 0.5, false, Some(Speaker::Them)));
        let blocks = lt.build_blocks();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].text.as_str(), "Bonjour");
        assert!(!lt.is_empty());
    }

    // SOU-256 AC5: finalized words are clickable (dictionary alias), the
    // provisional tail never is.
    #[test]
    fn only_finalized_live_words_are_clickable() {
        use slint::Model;
        let mut lt = LiveTranscript::new();
        lt.push_final(&seg(
            "Bonjour Kubernetes",
            0.0,
            1.0,
            true,
            Some(Speaker::Me),
        ));
        lt.push_tentative(&seg("provisoire", 1.1, 1.3, false, Some(Speaker::Me)));
        let blocks = lt.build_blocks();
        assert_eq!(blocks.len(), 1);
        let words: Vec<(String, String, bool)> = blocks[0]
            .words
            .iter()
            .map(|w| (w.text.to_string(), w.trailing_text.to_string(), w.clickable))
            .collect();
        assert!(words.contains(&("Kubernetes".into(), " ".into(), true)));
        assert!(words.contains(&("provisoire".into(), "".into(), false)));
        let rebuilt: String = words
            .iter()
            .flat_map(|(text, trailing, _)| [text.as_str(), trailing.as_str()])
            .collect();
        assert_eq!(rebuilt, blocks[0].text.as_str());
    }

    // No zipper: same-speaker segments retain insertion order even with
    // overlapping timestamps (monotonic_time clock regression).
    #[test]
    fn does_not_zipper_same_speaker_words_on_clock_regression() {
        let mut lt = LiveTranscript::new();
        lt.push_final(&seg("Word1", 5.0, 5.5, true, Some(Speaker::Me)));
        // Clock regression: start_time < previous end_time, but still same speaker.
        lt.push_final(&seg("Word2", 5.3, 5.8, true, Some(Speaker::Me)));
        let blocks = lt.build_blocks();
        // Should be grouped in one paragraph, in order.
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].text.as_str(), "Word1 Word2");
    }
}
