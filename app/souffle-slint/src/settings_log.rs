//! Virtualization math for the Settings "Journal en direct" module
//! (SOU-224). Same spacer-window pattern as `transcript.rs`: Rust owns the
//! full log tail as a line list, and only the lines overlapping the 140px
//! viewport (plus a margin) are ever mounted in Slint; everything scrolled
//! past is replaced by two estimated-height spacers. The window math itself
//! is shared with the transcript (`transcript::visible_window`).

use slint::SharedString;

use crate::transcript;

/// Matches `DiagnosticsSection`'s fixed `height: 140px` log Rectangle -
/// like the transcript's, the estimate only needs to be approximately
/// right, the margin absorbs the error.
pub const VIEWPORT_HEIGHT: f32 = 140.0;
pub const SCROLL_MARGIN: f32 = 3.0 * VIEWPORT_HEIGHT;

/// Rough wrap estimate for a 10.5px JetBrains Mono line in the log box
/// (see `transcript::CHARS_PER_LINE` for why a fixed count is fine here:
/// estimation error only skews the spacer/scrollbar proportion, mounted
/// lines are still laid out for real).
const CHARS_PER_LINE: usize = 90;
const LINE_HEIGHT_PX: f32 = 14.0;

/// Backing data for the virtualized log list - mirrors `TranscriptState`
/// in main.rs. `mounted_start == usize::MAX` means "nothing mounted yet",
/// so the first window computation always pushes a slice.
pub struct SettingsLogState {
    pub lines: Vec<SharedString>,
    pub offsets: Vec<f32>,
    pub mounted_start: usize,
    pub mounted_end: usize,
}

impl Default for SettingsLogState {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            offsets: vec![0.0],
            mounted_start: usize::MAX,
            mounted_end: usize::MAX,
        }
    }
}

/// Splits the raw tail string returned by `get_log_tail` into the line
/// model. An empty tail is zero lines (the Slint side then shows the
/// "Aucune entrée" placeholder, matching the old single-TextInput text).
pub fn split_tail(tail: &str) -> Vec<SharedString> {
    tail.lines().map(SharedString::from).collect()
}

fn estimate_line_height(line: &str) -> f32 {
    let rows = (line.chars().count().max(1) as f32 / CHARS_PER_LINE as f32).ceil();
    rows * LINE_HEIGHT_PX
}

/// Cumulative estimated y-offsets, same contract as
/// `transcript::compute_offsets`: one more entry than `lines`, always
/// non-decreasing, last entry = total estimated content height.
pub fn compute_offsets(lines: &[SharedString]) -> Vec<f32> {
    let mut offsets = Vec::with_capacity(lines.len() + 1);
    let mut y = 0.0f32;
    offsets.push(y);
    for line in lines {
        y += estimate_line_height(line);
        offsets.push(y);
    }
    offsets
}

pub fn visible_window(offsets: &[f32], scroll_top: f32) -> transcript::VisibleWindow {
    transcript::visible_window(offsets, scroll_top, VIEWPORT_HEIGHT, SCROLL_MARGIN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_tail_handles_empty_single_and_many() {
        assert!(split_tail("").is_empty());
        assert_eq!(split_tail("one line"), vec![SharedString::from("one line")]);
        let eighty = (0..80).map(|i| format!("line {i}")).collect::<Vec<_>>();
        assert_eq!(split_tail(&eighty.join("\n")).len(), 80);
        // A trailing newline must not create a phantom empty 81st line.
        assert_eq!(split_tail(&(eighty.join("\n") + "\n")).len(), 80);
    }

    #[test]
    fn window_at_top_of_an_80_line_tail_is_bounded() {
        let lines = split_tail(
            &(0..80)
                .map(|i| format!("2026-09-22T10:00:{i:02}Z INFO souffle: tick"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let offsets = compute_offsets(&lines);
        let win = visible_window(&offsets, 0.0);
        assert_eq!(win.start, 0);
        assert!(win.end < 80, "only a slice is mounted, got end={}", win.end);
        assert_eq!(win.spacer_before, 0.0);
        assert!(win.spacer_after > 0.0);
    }

    #[test]
    fn appending_lines_keeps_the_top_window_stable() {
        let short = split_tail(&vec!["a"; 60].join("\n"));
        let longer = split_tail(&vec!["a"; 70].join("\n"));
        let win_short = visible_window(&compute_offsets(&short), 0.0);
        let win_longer = visible_window(&compute_offsets(&longer), 0.0);
        // AC2: a poll that only appends past the window must not change the
        // mounted slice indices (so main.rs skips the model rebuild).
        assert_eq!(win_short.start, win_longer.start);
        assert_eq!(win_short.end, win_longer.end);
        assert_eq!(win_short.spacer_before, win_longer.spacer_before);
    }
}
