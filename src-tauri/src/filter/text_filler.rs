use std::sync::LazyLock;

use regex::Regex;

use super::{TextFilter, TextFilterKind};

/// Filler word pattern: English + French fillers, case-insensitive, word-boundary.
static FILLER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(uh+|um+|euh+|hm+|hmm+|ah+|hein)\b").expect("filler regex must compile")
});

pub struct FillerRemovalFilter;

impl FillerRemovalFilter {
    pub fn new() -> Self {
        Self
    }
}

impl TextFilter for FillerRemovalFilter {
    fn kind(&self) -> TextFilterKind {
        TextFilterKind::FillerRemoval
    }

    fn apply(&self, text: &str) -> String {
        let result = FILLER_PATTERN.replace_all(text, "");
        // Collapse whitespace left behind by removed fillers
        crate::engine::collapse_whitespace(&result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_english_fillers() {
        let f = FillerRemovalFilter::new();
        assert_eq!(f.apply("uh so um I think"), "so I think");
    }

    #[test]
    fn removes_french_fillers() {
        let f = FillerRemovalFilter::new();
        assert_eq!(f.apply("euh je pense que hein"), "je pense que");
    }

    #[test]
    fn case_insensitive() {
        let f = FillerRemovalFilter::new();
        assert_eq!(f.apply("UH so UM I think"), "so I think");
    }

    #[test]
    fn preserves_normal_text() {
        let f = FillerRemovalFilter::new();
        assert_eq!(f.apply("the umbrella is useful"), "the umbrella is useful");
    }

    #[test]
    fn handles_repeated_fillers() {
        let f = FillerRemovalFilter::new();
        assert_eq!(f.apply("umm uhhh so hmm yeah"), "so yeah");
    }

    #[test]
    fn empty_input() {
        let f = FillerRemovalFilter::new();
        assert_eq!(f.apply(""), "");
    }

    #[test]
    fn only_fillers_returns_empty() {
        let f = FillerRemovalFilter::new();
        assert_eq!(f.apply("uh um hmm"), "");
    }
}
