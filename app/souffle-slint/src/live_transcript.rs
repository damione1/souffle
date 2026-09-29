//! Live transcript state for a meeting in progress.
//!
//! The live view uses a deliberately simpler, incremental paragraph rule
//! than the canonical grouper in `souffle_schema::paragraphs`: a final
//! segment appends to the most recent tail paragraph of the same speaker
//! when the gap since that paragraph's last end is within the shared pause
//! threshold, and otherwise opens a new paragraph. The canonical grouper
//! splits a turn based on a *later* interrupter, so it cannot run
//! incrementally without re-cutting paragraphs already on screen. The
//! post-meeting view (`transcript.rs`) re-groups the finished meeting with
//! the canonical algorithm; only the pause threshold is shared.

use souffle_lib::engine::{Speaker, TranscriptionSegment};
use souffle_schema::paragraphs::PAUSE_THRESHOLD_SECONDS;

use crate::transcript::{build_words, speaker_fields};
use crate::{SpeakerRole, TranscriptBlock, TranscriptWord, timeline};

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

/// Tail window before a paragraph is committed (immutable).
const TAIL_WINDOW_S: f64 = 8.0;
/// Committed paragraphs kept in memory for the live view.
const LIVE_PARAGRAPH_WINDOW: usize = 30;

/// One in-progress or committed paragraph in the live view.
#[derive(Clone)]
pub struct LivePara {
    pub speaker: Option<Speaker>,
    /// `format_duration(first.start_time)` - fixed at creation, never updated.
    pub timestamp: String,
    pub start_time: f64,
    /// Last segment end-time seen (for pause detection).
    pub last_end: f64,
    /// Committed words, space-joined.
    pub text: String,
    /// Whether this paragraph is committed (past the tail window).
    pub committed: bool,
}

impl LivePara {
    fn new(seg: &TranscriptionSegment) -> Self {
        let trimmed = seg.text.trim().to_owned();
        Self {
            speaker: seg.speaker,
            timestamp: timeline::format_duration(seg.start_time),
            start_time: seg.start_time,
            last_end: seg.end_time.max(seg.start_time),
            text: trimmed,
            committed: false,
        }
    }

    fn append(&mut self, seg: &TranscriptionSegment) {
        let trimmed = seg.text.trim();
        if !trimmed.is_empty() {
            if !self.text.is_empty() {
                self.text.push(' ');
            }
            self.text.push_str(trimmed);
        }
        let seg_end = seg.end_time.max(seg.start_time);
        if seg_end > self.last_end {
            self.last_end = seg_end;
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
            text: text.clone().into(),
            words: live_words(&self.text, tentative_suffix),
            recording_session_index: -1,
            start_time: self.start_time as f32,
            end_label: "".into(),
            start_label: "".into(),
        }
    }
}

/// The word a lane's engine is still holding. It stays on screen, dimmed,
/// until the engine confirms it (the final replaces it) or opens the next
/// word: Damien, 2026-09-24, a pending word that vanished after 5 s and came
/// back at the end of the meeting read as a lost word. The engine always
/// emits a final for every pending word (next word, end of word, or flush),
/// so nothing is left dangling.
#[derive(Default)]
pub struct TentativeSlot {
    text: String,
}

impl TentativeSlot {
    fn set(&mut self, text: String) {
        self.text = text;
    }

    fn clear(&mut self) {
        self.text.clear();
    }

    fn active_text(&self) -> Option<&str> {
        (!self.text.is_empty()).then_some(self.text.as_str())
    }
}

/// The live transcript state machine. All mutations happen on the Slint
/// main thread (called from `invoke_from_event_loop`), so no locking needed.
pub struct LiveTranscript {
    /// Committed paragraphs (immutable once past tail window).
    pub committed: Vec<LivePara>,
    /// Tail: active paragraphs still within the tail window.
    pub tail: Vec<LivePara>,
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
            committed: Vec::new(),
            tail: Vec::new(),
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

        // Commit tail paragraphs that are past the tail window.
        let tail_cutoff = seg.start_time - TAIL_WINDOW_S;
        let mut newly_committed: Vec<LivePara> = Vec::new();
        let mut i = 0;
        while i < self.tail.len() {
            if self.tail[i].last_end <= tail_cutoff {
                let mut p = self.tail.remove(i);
                p.committed = true;
                newly_committed.push(p);
            } else {
                i += 1;
            }
        }
        self.committed.extend(newly_committed);
        // Keep committed list bounded.
        let max_committed = LIVE_PARAGRAPH_WINDOW.saturating_sub(1);
        if self.committed.len() > max_committed {
            let drop = self.committed.len() - max_committed;
            self.committed.drain(0..drop);
        }

        // Find or create the paragraph in the tail.
        let trimmed = seg.text.trim();
        if trimmed.is_empty() {
            return;
        }

        // Find the most recent tail paragraph of the same speaker to append to.
        let idx = self.tail.iter().rposition(|p| {
            p.speaker == seg.speaker && (seg.start_time - p.last_end) <= PAUSE_THRESHOLD_SECONDS
        });

        match idx {
            Some(i) => self.tail[i].append(seg),
            None => self.tail.push(LivePara::new(seg)),
        }
    }

    /// Push a tentative (non-final) word for a speaker lane.
    pub fn push_tentative(&mut self, seg: &TranscriptionSegment) {
        let text = seg.text.trim().to_owned();
        match seg.speaker {
            Some(Speaker::Me) => self.tentative_me.set(text),
            Some(Speaker::Them) => self.tentative_them.set(text),
            None => self.tentative_none.set(text),
        }
    }

    /// Build the Slint model: committed + tail paragraphs, with the active
    /// tentative text appended as a greyed suffix on the last paragraph of
    /// each speaker lane. Bounded to `LIVE_PARAGRAPH_WINDOW` entries.
    pub fn build_blocks(&self) -> Vec<TranscriptBlock> {
        // Merge committed + tail into a single chronological list, ordered
        // by paragraph start like the post-meeting grouper. Committed-then-
        // tail order alone is by commit time: a short interjection committed
        // before a still-running monologue jumped above it (SOU-256). The
        // sort is stable, so equal starts keep their creation order.
        let mut all: Vec<&LivePara> = self.committed.iter().chain(self.tail.iter()).collect();
        all.sort_by(|a, b| {
            a.start_time
                .partial_cmp(&b.start_time)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take the last LIVE_PARAGRAPH_WINDOW entries.
        let start = all.len().saturating_sub(LIVE_PARAGRAPH_WINDOW);
        let window = &all[start..];

        // Find the last paragraph index for each speaker to attach tentative.
        let last_me_idx = window.iter().rposition(|p| p.speaker == Some(Speaker::Me));
        let last_them_idx = window
            .iter()
            .rposition(|p| p.speaker == Some(Speaker::Them));
        let last_none_idx = window.iter().rposition(|p| p.speaker.is_none());

        let tentative_me = self.tentative_me.active_text();
        let tentative_them = self.tentative_them.active_text();
        let tentative_none = self.tentative_none.active_text();

        // Build orphan blocks for tentatives that have no committed paragraph yet.
        // These go after all committed paragraphs.
        let mut blocks: Vec<TranscriptBlock> = window
            .iter()
            .enumerate()
            .map(|(i, para)| {
                let suffix = match para.speaker {
                    Some(Speaker::Me) if Some(i) == last_me_idx => tentative_me,
                    Some(Speaker::Them) if Some(i) == last_them_idx => tentative_them,
                    None if Some(i) == last_none_idx => tentative_none,
                    _ => None,
                };
                para.to_slint_block(suffix)
            })
            .collect();

        // If a speaker has a tentative but no paragraph in the window, add an orphan block.
        if tentative_me.is_some() && last_me_idx.is_none() {
            let text = tentative_me.unwrap_or("");
            blocks.push(TranscriptBlock {
                is_session_break: false,
                has_speaker: true,
                speaker: SpeakerRole::Me,
                timestamp: "".into(),
                text: text.into(),
                words: live_words("", Some(text)),
                recording_session_index: -1,
                start_time: 0.0,
                end_label: "".into(),
                start_label: "".into(),
            });
        }
        if tentative_them.is_some() && last_them_idx.is_none() {
            let text = tentative_them.unwrap_or("");
            blocks.push(TranscriptBlock {
                is_session_break: false,
                has_speaker: true,
                speaker: SpeakerRole::Them,
                timestamp: "".into(),
                text: text.into(),
                words: live_words("", Some(text)),
                recording_session_index: -1,
                start_time: 0.0,
                end_label: "".into(),
                start_label: "".into(),
            });
        }

        blocks
    }

    /// True if there is nothing to show (no blocks, no tentative).
    pub fn is_empty(&self) -> bool {
        self.committed.is_empty()
            && self.tail.is_empty()
            && self.tentative_me.active_text().is_none()
            && self.tentative_them.active_text().is_none()
            && self.tentative_none.active_text().is_none()
    }

    /// Reset everything (called by `clear_live_transcript`).
    pub fn clear(&mut self) {
        self.committed.clear();
        self.tail.clear();
        self.tentative_me.clear();
        self.tentative_them.clear();
        self.tentative_none.clear();
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

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

    // AC1 equivalence: committed paragraphs are immutable.
    #[test]
    fn committed_paragraphs_are_immutable() {
        let mut lt = LiveTranscript::new();
        // First paragraph at t=0
        lt.push_final(&seg("Alpha", 0.0, 1.0, true, Some(Speaker::Me)));
        // Second paragraph well beyond tail window (>8s later) → first is committed.
        lt.push_final(&seg("Beta", 20.0, 21.0, true, Some(Speaker::Me)));
        // The committed block's text must still be just "Alpha".
        assert!(!lt.committed.is_empty());
        assert_eq!(lt.committed[0].text, "Alpha");
    }

    // SOU-256 (reported live by Damien): a short Me interjection during a
    // long Them monologue showed under it, then jumped above it once Me's
    // paragraph was committed first - committed paragraphs were listed
    // before tail ones regardless of time. Paragraphs are ordered by start
    // time, like the post-meeting grouper, and never move once shown.
    #[test]
    fn committing_a_paragraph_does_not_reorder_the_view() {
        let mut lt = LiveTranscript::new();
        let order = |lt: &LiveTranscript| -> Vec<SpeakerRole> {
            lt.build_blocks().iter().map(|b| b.speaker).collect()
        };
        // Them talks without a pause from 0:05 to 0:40 (one paragraph);
        // Me says one thing at 0:17, long enough ago by the end to have
        // left the tail window and been committed.
        let mut me_spoke = false;
        for i in 0..30 {
            let t = 5.0 + f64::from(i) * 1.2;
            lt.push_final(&seg("et encore", t, t + 1.0, true, Some(Speaker::Them)));
            if t >= 17.0 && !me_spoke {
                lt.push_final(&seg("Un deux trois", 17.0, 18.0, true, Some(Speaker::Me)));
                assert_eq!(order(&lt), vec![SpeakerRole::Them, SpeakerRole::Me]);
                me_spoke = true;
            }
        }
        assert!(
            !lt.committed.is_empty(),
            "Me's paragraph should be committed"
        );
        assert_eq!(order(&lt), vec![SpeakerRole::Them, SpeakerRole::Me]);
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
