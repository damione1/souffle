use strsim::normalized_levenshtein;

use super::soundex::soundex;
use super::{DictionaryEntry, TextFilter, TextFilterKind};

/// Maximum normalized Levenshtein distance for a fuzzy match.
const LEVENSHTEIN_THRESHOLD: f64 = 0.82; // similarity > 0.82 means distance < 0.18

/// Words shorter than this are never corrected: short function words ("va",
/// "de", "on") collide with almost anything phonetically and correcting them
/// does far more harm than good.
const MIN_WORD_LEN: usize = 3;

/// Minimum similarity for a phonetic-only (session term) match, on top of
/// Soundex equality.
const SESSION_SIMILARITY_FLOOR: f64 = 0.5;

pub struct DictionaryFilter {
    entries: Vec<DictionaryMatch>,
}

struct DictionaryMatch {
    term: String,
    term_lower: String,
    phonetic_code: Option<String>,
    /// Session-scoped terms (participant names, meeting jargon) were not
    /// chosen by the user, so they only match through Soundex plus a
    /// similarity floor — never through Levenshtein alone. This keeps
    /// injected names like "Martin" from rewriting ordinary words such as
    /// "matin" (similarity 0.83, above the fuzzy threshold).
    phonetic_only: bool,
}

/// Phonetic code used for matching: the user-provided pronunciation wins;
/// otherwise the term's own Soundex, except for digit-bearing terms ("V6",
/// "K8s") whose alphabetic Soundex would collide with unrelated short words.
fn derive_phonetic_code(term: &str, pronunciation: Option<&str>) -> Option<String> {
    if let Some(p) = pronunciation.map(str::trim).filter(|p| !p.is_empty()) {
        return soundex(p);
    }
    if term.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    soundex(term)
}

impl DictionaryFilter {
    /// `session_terms` are ephemeral additions for one recording session
    /// (participant names, meeting jargon); they match phonetically only.
    pub fn with_session_terms(entries: Vec<DictionaryEntry>, session_terms: &[String]) -> Self {
        let mut matches: Vec<DictionaryMatch> = entries
            .into_iter()
            .map(|e| {
                let term_lower = e.term.to_lowercase();
                let phonetic_code = derive_phonetic_code(&e.term, e.pronunciation.as_deref());
                DictionaryMatch {
                    term: e.term,
                    term_lower,
                    phonetic_code,
                    phonetic_only: false,
                }
            })
            .collect();
        for term in session_terms {
            let term_lower = term.to_lowercase();
            // A user entry for the same term wins over the session variant.
            if matches.iter().any(|m| m.term_lower == term_lower) {
                continue;
            }
            matches.push(DictionaryMatch {
                term: term.clone(),
                term_lower,
                phonetic_code: soundex(term),
                phonetic_only: true,
            });
        }
        Self { entries: matches }
    }

    fn find_replacement(&self, word: &str) -> Option<&str> {
        if word.chars().count() < MIN_WORD_LEN {
            return None;
        }
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

            let accepted = if entry.phonetic_only {
                soundex_match && similarity >= SESSION_SIMILARITY_FLOOR
            } else {
                // Accept if either condition is met
                similarity >= LEVENSHTEIN_THRESHOLD || soundex_match
            };
            if accepted {
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
            pronunciation: None,
            category: None,
            created_at: String::new(),
        }
    }

    fn entry_with_pronunciation(term: &str, pronunciation: &str) -> DictionaryEntry {
        DictionaryEntry {
            pronunciation: Some(pronunciation.to_string()),
            ..entry(term)
        }
    }

    #[test]
    fn corrects_close_misspelling() {
        let f = DictionaryFilter::with_session_terms(vec![entry("Kubernetes")], &[]);
        assert_eq!(f.apply("Kubernetis"), "Kubernetes");
    }

    #[test]
    fn preserves_exact_match() {
        let f = DictionaryFilter::with_session_terms(vec![entry("Docker")], &[]);
        assert_eq!(f.apply("Docker is great"), "Docker is great");
    }

    #[test]
    fn soundex_match_for_proper_nouns() {
        let f = DictionaryFilter::with_session_terms(vec![entry("Damien")], &[]);
        // "Damian" has the same Soundex as "Damien" (D550)
        assert_eq!(f.apply("Hello Damian"), "Hello Damien");
    }

    #[test]
    fn preserves_unrelated_words() {
        let f = DictionaryFilter::with_session_terms(vec![entry("Kubernetes")], &[]);
        assert_eq!(f.apply("the quick brown fox"), "the quick brown fox");
    }

    #[test]
    fn empty_dictionary_passthrough() {
        let f = DictionaryFilter::with_session_terms(vec![], &[]);
        assert_eq!(f.apply("hello world"), "hello world");
    }

    #[test]
    fn empty_input() {
        let f = DictionaryFilter::with_session_terms(vec![entry("Test")], &[]);
        assert_eq!(f.apply(""), "");
    }

    #[test]
    fn short_words_are_never_corrected() {
        // "va" collides with "V6" through alphabetic Soundex ("V000" both);
        // the 2-char guard must keep function words untouched.
        let f = DictionaryFilter::with_session_terms(vec![entry("V6")], &[]);
        assert_eq!(f.apply("il va faire"), "il va faire");

        let names = DictionaryFilter::with_session_terms(vec![entry("Damien")], &[]);
        assert_eq!(names.apply("de la part"), "de la part");
    }

    #[test]
    fn digit_terms_get_no_auto_phonetic_matching() {
        let f = DictionaryFilter::with_session_terms(vec![entry("V6")], &[]);
        // "vas" (3 chars, Soundex V000 too) must not become V6 either.
        assert_eq!(f.apply("tu vas bien"), "tu vas bien");
    }

    #[test]
    fn pronunciation_drives_phonetic_matching() {
        let f = DictionaryFilter::with_session_terms(
            vec![entry_with_pronunciation("V6", "vésix")],
            &[],
        );
        assert_eq!(f.apply("le vésix arrive"), "le V6 arrive");
        assert_eq!(f.apply("le vesix arrive"), "le V6 arrive");
        // Unrelated words with a different Soundex stay untouched.
        assert_eq!(f.apply("il va faire"), "il va faire");
        assert_eq!(f.apply("tu vas bien"), "tu vas bien");
    }

    #[test]
    fn blank_pronunciation_falls_back_to_term_soundex() {
        let f = DictionaryFilter::with_session_terms(
            vec![entry_with_pronunciation("Damien", "  ")],
            &[],
        );
        assert_eq!(f.apply("Hello Damian"), "Hello Damien");
    }

    #[test]
    fn session_terms_correct_phonetic_misses_only() {
        let f = DictionaryFilter::with_session_terms(vec![], &["Alice".to_string()]);
        // Same Soundex (A420), similar spelling: corrected.
        assert_eq!(f.apply("bonjour Alyce"), "bonjour Alice");
        assert_eq!(f.apply("bonjour Alice"), "bonjour Alice");
    }

    #[test]
    fn session_terms_never_match_through_levenshtein_alone() {
        // "matin" vs "Martin": similarity 0.83 (above the fuzzy threshold)
        // but different Soundex — a user entry would rewrite it, a session
        // term must not.
        let f = DictionaryFilter::with_session_terms(vec![], &["Martin".to_string()]);
        assert_eq!(f.apply("le matin venu"), "le matin venu");

        let user = DictionaryFilter::with_session_terms(vec![entry("Martin")], &[]);
        assert_eq!(user.apply("le matin venu"), "le Martin venu");
    }

    #[test]
    fn user_entry_wins_over_session_duplicate() {
        let f = DictionaryFilter::with_session_terms(
            vec![entry_with_pronunciation("V6", "vésix")],
            &["v6".to_string()],
        );
        assert_eq!(f.apply("le vésix arrive"), "le V6 arrive");
    }
}
