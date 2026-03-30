use strsim::normalized_levenshtein;

use super::soundex::soundex;
use super::{DictionaryEntry, TextFilter, TextFilterKind};

/// Maximum normalized Levenshtein distance for a fuzzy match.
const LEVENSHTEIN_THRESHOLD: f64 = 0.82; // similarity > 0.82 means distance < 0.18

pub struct DictionaryFilter {
    entries: Vec<DictionaryMatch>,
}

struct DictionaryMatch {
    term: String,
    term_lower: String,
    phonetic_code: Option<String>,
}

impl DictionaryFilter {
    pub fn new(entries: Vec<DictionaryEntry>) -> Self {
        let entries = entries
            .into_iter()
            .map(|e| {
                let term_lower = e.term.to_lowercase();
                let phonetic_code = e
                    .phonetic_code
                    .or_else(|| soundex(&e.term));
                DictionaryMatch {
                    term: e.term,
                    term_lower,
                    phonetic_code,
                }
            })
            .collect();
        Self { entries }
    }

    fn find_replacement(&self, word: &str) -> Option<&str> {
        let word_lower = word.to_lowercase();
        let word_soundex = soundex(word);

        let mut best_match: Option<(&str, f64)> = None;

        for entry in &self.entries {
            // Skip if the word already matches exactly
            if word_lower == entry.term_lower {
                return None;
            }

            // Check Levenshtein similarity
            let similarity = normalized_levenshtein(&word_lower, &entry.term_lower);

            // Check Soundex match
            let soundex_match = match (&word_soundex, &entry.phonetic_code) {
                (Some(w), Some(e)) => w == e,
                _ => false,
            };

            // Accept if either condition is met
            if similarity >= LEVENSHTEIN_THRESHOLD || soundex_match {
                let score = if soundex_match {
                    similarity + 0.1 // Boost soundex matches slightly
                } else {
                    similarity
                };
                if best_match.is_none() || score > best_match.unwrap().1 {
                    best_match = Some((&entry.term, score));
                }
            }
        }

        best_match.map(|(term, _)| term)
    }
}

impl TextFilter for DictionaryFilter {
    fn kind(&self) -> TextFilterKind {
        TextFilterKind::DictionaryCorrection
    }

    fn apply(&self, text: &str) -> String {
        if self.entries.is_empty() {
            return text.to_string();
        }

        let mut result = String::with_capacity(text.len());
        let mut chars = text.char_indices().peekable();

        while let Some(&(start, ch)) = chars.peek() {
            if ch.is_alphanumeric() {
                // Collect the full word
                let mut end = start;
                while let Some(&(i, c)) = chars.peek() {
                    if c.is_alphanumeric() || c == '\'' || c == '-' {
                        end = i + c.len_utf8();
                        chars.next();
                    } else {
                        break;
                    }
                }
                let word = &text[start..end];
                if let Some(replacement) = self.find_replacement(word) {
                    result.push_str(replacement);
                } else {
                    result.push_str(word);
                }
            } else {
                result.push(ch);
                chars.next();
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(term: &str) -> DictionaryEntry {
        DictionaryEntry {
            id: 0,
            term: term.to_string(),
            phonetic_code: None,
            category: None,
            created_at: String::new(),
        }
    }

    #[test]
    fn corrects_close_misspelling() {
        let f = DictionaryFilter::new(vec![entry("Kubernetes")]);
        assert_eq!(f.apply("Kubernetis"), "Kubernetes");
    }

    #[test]
    fn preserves_exact_match() {
        let f = DictionaryFilter::new(vec![entry("Docker")]);
        assert_eq!(f.apply("Docker is great"), "Docker is great");
    }

    #[test]
    fn soundex_match_for_proper_nouns() {
        let f = DictionaryFilter::new(vec![entry("Damien")]);
        // "Damian" has the same Soundex as "Damien" (D550)
        assert_eq!(f.apply("Hello Damian"), "Hello Damien");
    }

    #[test]
    fn preserves_unrelated_words() {
        let f = DictionaryFilter::new(vec![entry("Kubernetes")]);
        assert_eq!(f.apply("the quick brown fox"), "the quick brown fox");
    }

    #[test]
    fn empty_dictionary_passthrough() {
        let f = DictionaryFilter::new(vec![]);
        assert_eq!(f.apply("hello world"), "hello world");
    }

    #[test]
    fn empty_input() {
        let f = DictionaryFilter::new(vec![entry("Test")]);
        assert_eq!(f.apply(""), "");
    }
}
