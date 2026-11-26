//! Render windowing of the completed meeting's transcript (SOU-187 AC15),
//! separate from `main.rs` so the design fixture drives the same code.

use std::cell::RefCell;
use std::rc::Rc;

use crate::{MainWindow, TranscriptBlock, transcript};

/// Upper bound of the meeting page's scroll region, which the transcript
/// list scrolls in - the windowing math below only needs to be approximately
/// right (it pads with `TRANSCRIPT_SCROLL_MARGIN` on each side), not
/// pixel-exact.
const TRANSCRIPT_VIEWPORT_HEIGHT: f32 = 900.0;
const TRANSCRIPT_SCROLL_MARGIN: f32 = 1.5 * TRANSCRIPT_VIEWPORT_HEIGHT;

/// Backing data for the virtualized transcript list: the full block list
/// plus precomputed cumulative height estimates (see
/// `transcript::compute_offsets`). `MeetingDetail`'s `transcript-blocks`
/// property only ever holds the current visible slice of this, never the
/// whole thing - that's the actual virtualization.
pub(crate) struct TranscriptState {
    pub(crate) source: Vec<TranscriptBlock>,
    pub(crate) blocks: Vec<TranscriptBlock>,
    pub(crate) offsets: Vec<f32>,
    pub(crate) mounted_start: usize,
    pub(crate) mounted_end: usize,
}

/// Installs a meeting's full block list and mounts the slice at the top.
pub(crate) fn mount_transcript(
    window: &MainWindow,
    blocks: Vec<TranscriptBlock>,
    transcript_state: &Rc<RefCell<Option<TranscriptState>>>,
) {
    let offsets = transcript::compute_offsets(&blocks);
    window.set_meeting_detail_transcript_preview(transcript::preview_text(&blocks).into());
    *transcript_state.borrow_mut() = Some(TranscriptState {
        source: blocks.clone(),
        blocks,
        offsets,
        mounted_start: usize::MAX,
        mounted_end: usize::MAX,
    });
    window.invoke_reset_meeting_detail_transcript_scroll();
    update_transcript_window(window, transcript_state, 0.0);
}

/// Recomputes which slice of `transcript_state`'s blocks should be mounted
/// for `scroll_top` (px scrolled down from the top) and pushes it into
/// `MeetingDetail`'s properties, but only when the slice actually changed -
/// small scroll steps inside the pre-mounted margin must not rebuild the
/// Slint model.
pub(crate) fn update_transcript_window(
    window: &MainWindow,
    transcript_state: &Rc<RefCell<Option<TranscriptState>>>,
    scroll_top: f32,
) {
    let mut guard = transcript_state.borrow_mut();
    let Some(state) = guard.as_mut() else {
        return;
    };
    let win = transcript::visible_window(
        &state.offsets,
        scroll_top,
        TRANSCRIPT_VIEWPORT_HEIGHT,
        TRANSCRIPT_SCROLL_MARGIN,
    );
    if win.start == state.mounted_start && win.end == state.mounted_end {
        return;
    }
    state.mounted_start = win.start;
    state.mounted_end = win.end;
    let slice = state.blocks[win.start..win.end].to_vec();
    let mounted = slice.len();
    let total = state.blocks.len();
    window.set_meeting_detail_transcript_blocks(Rc::new(slint::VecModel::from(slice)).into());
    window.set_meeting_detail_transcript_spacer_before(win.spacer_before);
    window.set_meeting_detail_transcript_spacer_after(win.spacer_after);
    // AC15's "active node counter" evidence: this stays small and bounded
    // even for a meeting with thousands of paragraphs - see
    // `transcript::tests::visible_window_slices_a_huge_transcript_to_a_bounded_count`
    // for the automated version of this same claim.
    eprintln!("transcript window: {mounted}/{total} blocks mounted (SOU-187 AC15)");
}
