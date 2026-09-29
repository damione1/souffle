use super::{TextFilter, TextFilterKind};

/// Wraps existing `crate::engine::collapse_whitespace()` — DRY, no duplication.
pub struct WhitespaceNormFilter;

impl TextFilter for WhitespaceNormFilter {
    fn kind(&self) -> TextFilterKind {
        TextFilterKind::WhitespaceNormalization
    }

    fn apply(&self, text: &str) -> String {
        crate::engine::collapse_whitespace(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_multiple_spaces() {
        let f = WhitespaceNormFilter;
        assert_eq!(f.apply("hello   world"), "hello world");
    }

    #[test]
    fn trims_leading_trailing() {
        let f = WhitespaceNormFilter;
        assert_eq!(f.apply("  hello  "), "hello");
    }

    #[test]
    fn empty_input() {
        let f = WhitespaceNormFilter;
        assert_eq!(f.apply(""), "");
    }
}
