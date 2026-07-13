use strsim::normalized_levenshtein;

use super::soundex::soundex;
use super::{DictionaryEntry, TextFilter, TextFilterKind};

/// Maximum normalized Levenshtein distance for a fuzzy match.
const LEVENSHTEIN_THRESHOLD: f64 = 0.82; // similarity > 0.82 means distance < 0.18

/// Words shorter than this are never corrected: short function words ("va",
/// "de", "on") collide with almost anything phonetically and correcting them
/// does far more harm than good.
const MIN_WORD_LEN: usize = 3;

/// Words shorter than this are never corrected through a Soundex-only match
/// (no strong spelling similarity backing it up). Standard Soundex reduces a
/// word to a first letter plus 3 digits, so short tokens run out of letters
/// to differentiate themselves and collide with many unrelated codes. The
/// Levenshtein spelling-correction path below is unaffected by this floor.
const MIN_PHONETIC_WORD_LEN: usize = 4;

/// Minimum normalized similarity, on top of Soundex equality, required for a
/// *derived* phonetic match: one computed from the term's own spelling (or a
/// session term), not from an explicit user pronunciation. Soundex equality
/// alone is a weak signal, collapsing "dans" and "Denis" to the same code
/// (D520); this floor keeps that kind of orthographically unrelated pair
/// from matching while still allowing real misheard spellings through, e.g.
/// "Delphine" transcribed as "Delfine" (similarity 0.75).
const PHONETIC_SIMILARITY_FLOOR: f64 = 0.65;

/// High-frequency French and English function words that a *derived*
/// phonetic match must never replace. These are exactly the words most
/// likely to collide with a name added to the dictionary purely by
/// coincidence (short, common, phonetically generic). An explicit
/// user-typed pronunciation is the only thing allowed to override this list
/// (see `DictionaryMatch::explicit_pronunciation`): that's a deliberate
/// instruction from the user, not an accidental Soundex collision.
///
/// Extend this list as new false positives get reported; it is intentionally
/// a curated sample of common short words, not an exhaustive stopword corpus.
const PROTECTED_STOPWORDS: &[&str] = &[
    // French
    "dans", "donc", "des", "dont", "doit", "deux", "par", "pour", "pas", "peu", "peut", "plus",
    "avec", "sans", "sous", "sur", "vers", "chez", "mais", "car", "que", "qui", "quoi", "quand",
    "comme", "alors", "aussi", "bien", "bon", "cette", "cet", "ces", "ses", "son", "sa", "ton",
    "leur", "leurs", "nos", "vos", "tout", "tous", "toute", "toutes", "etre", "avoir", "fait",
    "faire", "dit", "dire", "cela", "ceci", "ainsi", "puis", "encore", "toujours", "jamais",
    "rien", "ici",
    // English
    "the", "then", "than", "this", "that", "these", "those", "with", "from", "have", "been",
    "were", "when", "what", "which", "who", "whom", "where", "there", "their", "they", "them",
    "will", "would", "could", "should", "just", "very", "also", "into", "onto", "over", "under",
    "about", "after", "before",
];

fn is_protected_stopword(word_lower: &str) -> bool {
    PROTECTED_STOPWORDS.contains(&word_lower)
}

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
    /// True when `phonetic_code` came from a pronunciation the user typed in
    /// explicitly, rather than being derived from the term's own spelling.
    /// A deliberate instruction ("correct this sound to this term") is
    /// exempt from the stopword list and similarity floor below: the user
    /// asked for it, so a Soundex hit on it isn't a coincidence.
    explicit_pronunciation: bool,
}

/// Phonetic code used for matching: the user-provided pronunciation wins;
/// otherwise the term's own Soundex, except for digit-bearing terms ("V6",
/// "K8s") whose alphabetic Soundex would collide with unrelated short words.
/// Returns the code plus whether it came from an explicit pronunciation.
fn derive_phonetic_code(term: &str, pronunciation: Option<&str>) -> (Option<String>, bool) {
    if let Some(p) = pronunciation.map(str::trim).filter(|p| !p.is_empty()) {
        return (soundex(p), true);
    }
    if term.chars().any(|c| c.is_ascii_digit()) {
        return (None, false);
    }
    (soundex(term), false)
}

impl DictionaryFilter {
    /// `session_terms` are ephemeral additions for one recording session
    /// (participant names, meeting jargon); they match phonetically only.
    pub fn with_session_terms(entries: Vec<DictionaryEntry>, session_terms: &[String]) -> Self {
        let mut matches: Vec<DictionaryMatch> = entries
            .into_iter()
            .map(|e| {
                let term_lower = e.term.to_lowercase();
                let (phonetic_code, explicit_pronunciation) =
                    derive_phonetic_code(&e.term, e.pronunciation.as_deref());
                DictionaryMatch {
                    term: e.term,
                    term_lower,
                    phonetic_code,
                    phonetic_only: false,
                    explicit_pronunciation,
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
                explicit_pronunciation: false,
            });
        }
        Self { entries: matches }
    }

    fn find_replacement(&self, word: &str) -> Option<&str> {
        let word_char_count = word.chars().count();
        if word_char_count < MIN_WORD_LEN {
            return None;
        }
        let word_lower = word.to_lowercase();
        let word_soundex = soundex(word);
        let is_stopword = is_protected_stopword(&word_lower);

        let mut best_match: Option<(&str, f64)> = None;

        for entry in &self.entries {
            // Skip if the word already matches exactly
            if word_lower == entry.term_lower {
                return None;
            }

            // A protected stopword can only be corrected by an explicit
            // user-typed pronunciation, never by a coincidental Soundex or
            // fuzzy-spelling hit.
            if is_stopword && !entry.explicit_pronunciation {
                continue;
            }

            // Check Levenshtein similarity
            let similarity = normalized_levenshtein(&word_lower, &entry.term_lower);

            // Check Soundex match
            let soundex_match = match (&word_soundex, &entry.phonetic_code) {
                (Some(w), Some(e)) => w == e,
                _ => false,
            };

            let accepted = if entry.explicit_pronunciation {
                // The user typed this pronunciation on purpose: honor it
                // exactly as before, with no extra length/similarity floor.
                similarity >= LEVENSHTEIN_THRESHOLD || soundex_match
            } else {
                let derived_phonetic_match = soundex_match
                    && word_char_count >= MIN_PHONETIC_WORD_LEN
                    && similarity >= PHONETIC_SIMILARITY_FLOOR;
                if entry.phonetic_only {
                    // Session terms never get the plain-spelling path either:
                    // they were not chosen by the user (see struct doc).
                    derived_phonetic_match
                } else {
                    similarity >= LEVENSHTEIN_THRESHOLD || derived_phonetic_match
                }
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

    // ── Overmatch regression: "Dans" -> "Paris" (production report) ──
    //
    // Root cause: standard Soundex keeps the literal first letter, so
    // "Dans" (D520) and "Paris" (P620) never collide; that exact pair
    // cannot reproduce through this filter. The real defect is structural:
    // a *derived* (non-alias) dictionary entry accepted a Soundex match with
    // no similarity floor at all, so any common word sharing a Soundex code
    // with a dictionary name got replaced regardless of how different the
    // spelling was. "Dans" and a "Denis" entry both hash to D520
    // (similarity only 0.6) and reproduce the same class of bug the report
    // describes. These tests pin both the specific reported pair and the
    // general mechanism.

    #[test]
    fn dans_is_never_replaced_by_paris() {
        // Confirms the reported pair does not collide via Soundex, and stays
        // untouched end-to-end regardless.
        assert_ne!(soundex("Dans"), soundex("Paris"));
        let f = DictionaryFilter::with_session_terms(vec![entry("Paris")], &[]);
        assert_eq!(f.apply("Dans la maison"), "Dans la maison");
        assert_eq!(f.apply("dans la maison"), "dans la maison");
    }

    #[test]
    fn stopword_blocks_derived_phonetic_collision() {
        // The actual matching path behind the report: a derived (no
        // pronunciation) dictionary entry whose Soundex organically collides
        // with a common word. "Denis" and "Dans" both hash to D520.
        assert_eq!(soundex("Dans"), soundex("Denis"));
        let f = DictionaryFilter::with_session_terms(vec![entry("Denis")], &[]);
        assert_eq!(f.apply("Dans la maison"), "Dans la maison");

        // Same collision through a session term (e.g. a calendar attendee
        // named "Denis") must be blocked too.
        let session = DictionaryFilter::with_session_terms(vec![], &["Denis".to_string()]);
        assert_eq!(session.apply("Dans la maison"), "Dans la maison");
    }

    #[test]
    fn explicit_alias_overrides_stopword_protection() {
        // An explicit pronunciation is a deliberate user instruction, so it
        // is allowed to touch a protected stopword even though a derived
        // match on the same word would be blocked.
        let f = DictionaryFilter::with_session_terms(
            vec![entry_with_pronunciation("Denis", "dans")],
            &[],
        );
        assert_eq!(f.apply("Dans la maison"), "Denis la maison");
    }

    #[test]
    fn short_word_blocks_soundex_only_match() {
        // "toi" (3 chars, not on the stopword list) meets MIN_WORD_LEN but
        // not MIN_PHONETIC_WORD_LEN; a derived entry relying on Soundex
        // alone must not touch it.
        let f = DictionaryFilter::with_session_terms(vec![entry("Toy")], &[]);
        assert_eq!(soundex("toi"), soundex("Toy"));
        assert_eq!(f.apply("regarde toi"), "regarde toi");
    }

    #[test]
    fn legitimate_phonetic_correction_still_fires() {
        // "Delphine" misheard as "Delfine" (soundex match, similarity 0.75):
        // above the new floor, so the correction still applies.
        assert_eq!(soundex("Delphine"), soundex("Delfine"));
        let f = DictionaryFilter::with_session_terms(vec![entry("Delphine")], &[]);
        assert_eq!(f.apply("bonjour Delfine"), "bonjour Delphine");

        // Case of the original word is not preserved by design (the
        // dictionary term's own casing always wins).
        let session = DictionaryFilter::with_session_terms(vec![], &["Delphine".to_string()]);
        assert_eq!(session.apply("bonjour delfine"), "bonjour Delphine");
    }

    #[test]
    fn low_similarity_soundex_match_is_rejected_even_off_stopword_list() {
        // "marche" (to walk) is not on the stopword list, so this isolates
        // the PHONETIC_SIMILARITY_FLOOR guard from stopword protection: the
        // same weak-similarity shape as "dans"/"denis" must still be
        // rejected on its own merits.
        let f = DictionaryFilter::with_session_terms(vec![entry("Mauriac")], &[]);
        assert_eq!(soundex("marche"), soundex("Mauriac"));
        assert!(normalized_levenshtein("marche", "mauriac") < PHONETIC_SIMILARITY_FLOOR);
        assert_eq!(f.apply("il marche vite"), "il marche vite");
    }
}
