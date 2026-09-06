use strsim::normalized_levenshtein;

use super::learned_pair::{PHONETIC_SIMILARITY_FLOOR, is_protected_stopword};
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

pub struct DictionaryFilter {
    entries: Vec<DictionaryMatch>,
    exact_aliases: Vec<ExactAlias>,
}

struct ExactAlias {
    alias_lower: String,
    term: String,
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
        let primary = p
            .split(',')
            .map(str::trim)
            .find(|alias| !alias.is_empty())
            .unwrap_or(p);
        return (soundex(primary), true);
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
        Self::with_session_hints(entries, session_terms, &[])
    }

    /// Session terms plus explicit misspelling-to-term pairs from live edits.
    pub fn with_session_hints(
        entries: Vec<DictionaryEntry>,
        session_terms: &[String],
        session_corrections: &[super::session_terms::SessionCorrection],
    ) -> Self {
        let mut exact_aliases = Vec::new();
        let mut matches: Vec<DictionaryMatch> = Vec::with_capacity(entries.len());
        for e in entries {
            for alias in super::pronunciation_aliases(&e.term, e.pronunciation.as_deref()) {
                exact_aliases.push(ExactAlias {
                    alias_lower: alias.to_lowercase(),
                    term: e.term.clone(),
                });
            }
            let term_lower = e.term.to_lowercase();
            let (phonetic_code, explicit_pronunciation) =
                derive_phonetic_code(&e.term, e.pronunciation.as_deref());
            matches.push(DictionaryMatch {
                term: e.term,
                term_lower,
                phonetic_code,
                phonetic_only: false,
                explicit_pronunciation,
            });
        }
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
        for correction in session_corrections {
            let miss = correction.misspelling.trim();
            if !miss.is_empty() && miss.to_lowercase() != correction.term.to_lowercase() {
                if !is_protected_stopword(&miss.to_lowercase()) {
                    exact_aliases.push(ExactAlias {
                        alias_lower: miss.to_lowercase(),
                        term: correction.term.clone(),
                    });
                }
            }
            let term_lower = correction.term.to_lowercase();
            if matches.iter().any(|m| m.term_lower == term_lower) {
                continue;
            }
            matches.push(DictionaryMatch {
                term: correction.term.clone(),
                term_lower,
                phonetic_code: soundex(&correction.misspelling),
                phonetic_only: false,
                // Learned from a live edit, not typed as a pronunciation:
                // it goes through the derived floors, not the explicit bypass.
                explicit_pronunciation: false,
            });
        }
        exact_aliases.sort_by(|a, b| {
            b.alias_lower
                .chars()
                .count()
                .cmp(&a.alias_lower.chars().count())
        });
        Self {
            entries: matches,
            exact_aliases,
        }
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

    fn apply_exact_aliases(&self, text: &str) -> String {
        if self.exact_aliases.is_empty() {
            return text.to_string();
        }

        let mut result = String::with_capacity(text.len());
        let mut byte_index = 0;
        while byte_index < text.len() {
            if at_word_start(text, byte_index) {
                let mut matched = false;
                for alias in &self.exact_aliases {
                    if let Some(end) = match_alias_at(text, byte_index, &alias.alias_lower) {
                        result.push_str(&alias.term);
                        byte_index = end;
                        matched = true;
                        break;
                    }
                }
                if matched {
                    continue;
                }
            }
            let ch = text[byte_index..]
                .chars()
                .next()
                .expect("byte_index on char boundary");
            result.push(ch);
            byte_index += ch.len_utf8();
        }
        result
    }

    fn apply_fuzzy(&self, text: &str) -> String {
        let mut result = String::with_capacity(text.len());
        let mut chars = text.char_indices().peekable();

        while let Some(&(start, ch)) = chars.peek() {
            if ch.is_alphanumeric() {
                let mut end = start;
                while let Some(&(i, c)) = chars.peek() {
                    if is_word_char(c) {
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

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '\'' || c == '-'
}

fn at_word_start(text: &str, byte_index: usize) -> bool {
    let Some(ch) = text[byte_index..].chars().next() else {
        return false;
    };
    if !is_word_char(ch) {
        return false;
    }
    byte_index == 0
        || text[..byte_index]
            .chars()
            .next_back()
            .is_none_or(|prev| !is_word_char(prev))
}

fn match_alias_at(text: &str, byte_start: usize, alias_lower: &str) -> Option<usize> {
    let mut consumed = 0;
    let mut text_chars = text[byte_start..].chars();
    for alias_ch in alias_lower.chars() {
        let ch = text_chars.next()?;
        if !char_eq_ignore_case(ch, alias_ch) {
            return None;
        }
        consumed += ch.len_utf8();
    }
    if text_chars.next().is_some_and(is_word_char) {
        return None;
    }
    Some(byte_start + consumed)
}

fn char_eq_ignore_case(text_ch: char, alias_lower_ch: char) -> bool {
    if text_ch == alias_lower_ch {
        return true;
    }
    let mut lower = text_ch.to_lowercase();
    lower.next() == Some(alias_lower_ch) && lower.next().is_none()
}

impl TextFilter for DictionaryFilter {
    fn kind(&self) -> TextFilterKind {
        TextFilterKind::DictionaryCorrection
    }

    fn apply(&self, text: &str) -> String {
        if self.entries.is_empty() {
            return text.to_string();
        }
        if self.exact_aliases.is_empty() {
            self.apply_fuzzy(text)
        } else {
            self.apply_fuzzy(&self.apply_exact_aliases(text))
        }
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
    fn session_corrections_rewrite_live_misspellings() {
        use crate::filter::session_terms::SessionCorrection;
        let f = DictionaryFilter::with_session_hints(
            vec![],
            &[],
            &[SessionCorrection {
                misspelling: "Kubernetis".to_string(),
                term: "Kubernetes".to_string(),
            }],
        );
        assert_eq!(f.apply("Kubernetis cluster"), "Kubernetes cluster");
    }

    #[test]
    fn learned_session_correction_does_not_override_stopword_protection() {
        use crate::filter::session_terms::SessionCorrection;
        let f = DictionaryFilter::with_session_hints(
            vec![],
            &[],
            &[SessionCorrection {
                misspelling: "pour".to_string(),
                term: "Pierre".to_string(),
            }],
        );
        assert_eq!(f.apply("par ici"), "par ici");
        assert_eq!(f.apply("pour la revue"), "pour la revue");
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

    #[test]
    fn exact_pronunciation_aliases_replace_single_and_multi_word() {
        let f = DictionaryFilter::with_session_terms(
            vec![entry_with_pronunciation("V6", "vésix, vee six")],
            &[],
        );
        assert_eq!(f.apply("le vésix arrive"), "le V6 arrive");
        assert_eq!(f.apply("le vee six arrive"), "le V6 arrive");
        assert_eq!(f.apply("le VEE SIX arrive"), "le V6 arrive");
        assert_eq!(f.apply("le vesix arrive"), "le V6 arrive");
    }

    #[test]
    fn exact_pronunciation_alias_replaces_multi_word_phrase() {
        let f = DictionaryFilter::with_session_terms(
            vec![entry_with_pronunciation("FluidVoice", "fluid boys")],
            &[],
        );
        assert_eq!(f.apply("fluid boys"), "FluidVoice");
        assert_eq!(f.apply("try fluid boys today"), "try FluidVoice today");
        assert_eq!(f.apply("FLUID BOYS"), "FluidVoice");
    }

    #[test]
    fn exact_alias_ignores_self_equal_and_empty_pieces() {
        let f = DictionaryFilter::with_session_terms(
            vec![entry_with_pronunciation("V6", "V6, , vésix")],
            &[],
        );
        assert_eq!(f.apply("le vésix"), "le V6");
        assert_eq!(f.apply("le V6"), "le V6");
    }

    #[test]
    fn exact_alias_respects_word_boundaries() {
        let f = DictionaryFilter::with_session_terms(
            vec![entry_with_pronunciation("V6", "vee six")],
            &[],
        );
        assert_eq!(f.apply("vee sixteen"), "vee sixteen");
    }

    #[test]
    fn longest_exact_alias_wins_on_overlap() {
        let f = DictionaryFilter::with_session_terms(
            vec![
                entry_with_pronunciation("FluidVoice", "fluid boys"),
                entry_with_pronunciation("Xbox", "boys"),
            ],
            &[],
        );
        assert_eq!(f.apply("fluid boys"), "FluidVoice");
        assert_eq!(f.apply("the boys"), "the Xbox");
    }

    #[test]
    fn session_correction_misspelling_is_exact_even_for_known_term() {
        use crate::filter::session_terms::SessionCorrection;
        let f = DictionaryFilter::with_session_hints(
            vec![entry("Kubernetes")],
            &[],
            &[SessionCorrection {
                misspelling: "kube er neties".to_string(),
                term: "Kubernetes".to_string(),
            }],
        );
        assert_eq!(
            f.apply("the kube er neties cluster"),
            "the Kubernetes cluster"
        );
    }
}
