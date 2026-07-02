use super::{TextFilter, TextFilterKind};

/// Collapse 3+ consecutive repetitions of ANY word to a single occurrence.
/// Implemented without regex backreferences (not supported by Rust `regex` crate).
pub struct StutterCollapseFilter;

impl StutterCollapseFilter {
    pub fn new() -> Self {
        Self
    }
}

impl TextFilter for StutterCollapseFilter {
    fn kind(&self) -> TextFilterKind {
        TextFilterKind::StutterCollapse
    }

    fn apply(&self, text: &str) -> String {
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.is_empty() {
            return String::new();
        }

        let mut result: Vec<&str> = Vec::with_capacity(words.len());
        let mut i = 0;

        while i < words.len() {
            let word = words[i];
            // Count consecutive repetitions (case-insensitive)
            let mut count = 1;
            while i + count < words.len() && words[i + count].eq_ignore_ascii_case(word) {
                count += 1;
            }

            if count >= 3 {
                // Collapse 3+ repetitions to one
                result.push(word);
                i += count;
            } else {
                result.push(word);
                i += 1;
            }
        }

        result.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_triple_repeat() {
        let f = StutterCollapseFilter::new();
        assert_eq!(f.apply("the the the cat"), "the cat");
    }

    #[test]
    fn preserves_double_repeat() {
        let f = StutterCollapseFilter::new();
        assert_eq!(f.apply("the the cat"), "the the cat");
    }

    #[test]
    fn collapses_long_repeat() {
        let f = StutterCollapseFilter::new();
        assert_eq!(f.apply("I I I I I want"), "I want");
    }

    #[test]
    fn preserves_normal_text() {
        let f = StutterCollapseFilter::new();
        assert_eq!(f.apply("the quick brown fox"), "the quick brown fox");
    }

    #[test]
    fn case_insensitive() {
        let f = StutterCollapseFilter::new();
        assert_eq!(f.apply("The the THE cat"), "The cat");
    }

    #[test]
    fn empty_input() {
        let f = StutterCollapseFilter::new();
        assert_eq!(f.apply(""), "");
    }
}
