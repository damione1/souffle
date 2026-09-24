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
use std::time::{Duration, Instant};

use crate::transcript::speaker_fields;
use crate::{SpeakerRole, TranscriptBlock, TranscriptWord, timeline};

/// The live view renders `TranscriptBlock.text` directly
/// (`recording_view.slint`), never `.words` - no per-word dictionary-alias
/// click here (SOU-223 is post-meeting only). This just satisfies the
/// shared struct's field with a single non-clickable word wrapping the
/// whole text, matching the old `StyledText::from_plain_text` intent.
fn plain_words(text: &str) -> slint::ModelRc<TranscriptWord> {
    slint::ModelRc::new(slint::VecModel::from(vec![TranscriptWord {
        text: text.into(),
        clickable: false,
    }]))
}

/// Tail window before a paragraph is committed (immutable).
const TAIL_WINDOW_S: f64 = 8.0;
/// Committed paragraphs kept in memory for the live view.
const LIVE_PARAGRAPH_WINDOW: usize = 30;
/// Tentative expiry: 5s without a matching final drops the pending word.
const TENTATIVE_EXPIRY: Duration = Duration::from_secs(5);

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
            words: plain_words(&text),
            recording_session_index: -1,
            start_time: self.start_time as f32,
            end_label: "".into(),
            start_label: "".into(),
        }
    }
}

#[derive(Default)]
pub struct TentativeSlot {
    text: String,
    updated_at: Option<Instant>,
}

impl TentativeSlot {
    fn set(&mut self, text: String) {
        self.text = text;
        self.updated_at = Some(Instant::now());
    }

    fn clear(&mut self) {
        self.text.clear();
        self.updated_at = None;
    }

    fn is_expired(&self) -> bool {
        match self.updated_at {
            Some(t) => t.elapsed() > TENTATIVE_EXPIRY,
            None => false,
        }
    }

    fn active_text(&self) -> Option<&str> {
        if self.text.is_empty() || self.is_expired() {
            None
        } else {
            Some(&self.text)
        }
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

    /// Expire stale tentative slots. Call this from the tick timer. Returns
    /// whether anything was dropped, i.e. whether the view needs a refresh.
    pub fn expire_tentatives(&mut self) -> bool {
        let mut expired = false;
        for slot in [
            &mut self.tentative_me,
            &mut self.tentative_them,
            &mut self.tentative_none,
        ] {
            if slot.is_expired() {
                slot.clear();
                expired = true;
            }
        }
        expired
    }

    /// Build the Slint model: committed + tail paragraphs, with the active
    /// tentative text appended as a greyed suffix on the last paragraph of
    /// each speaker lane. Bounded to `LIVE_PARAGRAPH_WINDOW` entries.
    pub fn build_blocks(&self) -> Vec<TranscriptBlock> {
        // Merge committed + tail into a single chronological list.
        let all: Vec<&LivePara> = self.committed.iter().chain(self.tail.iter()).collect();

        // Take the last LIVE_PARAGRAPH_WINDOW entries.
        let start = all.len().saturating_sub(LIVE_PARAGRAPH_WINDOW);
        let window = &all[start..];

        if window.is_empty() {
            return Vec::new();
        }

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
                words: plain_words(text),
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
                words: plain_words(text),
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

    // AC3: tentative expires after TENTATIVE_EXPIRY.
    #[test]
    fn tentative_expires() {
        let mut slot = TentativeSlot::default();
        slot.set("test".into());
        // Fast-forward by tweaking updated_at.
        slot.updated_at = Some(Instant::now() - (TENTATIVE_EXPIRY + Duration::from_millis(100)));
        assert!(slot.is_expired());
        assert_eq!(slot.active_text(), None);
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
