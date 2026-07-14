//! Session-scoped transcription hints derived from meeting context
//! (calendar participants, event title and description). These terms feed
//! the dictionary filter for one recording session only and are never
//! persisted to the user's dictionary.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Upper bound on derived terms: invitation bodies can be huge
/// (videoconference boilerplate), and every term costs a comparison per
/// transcribed word.
const MAX_SESSION_TERMS: usize = 50;

/// Minimum token length; shorter tokens collide with function words.
const MIN_TOKEN_LEN: usize = 3;

/// An explicit misspelling-to-term pair registered when the user edits live
/// transcript text. Applied through the dictionary filter for the rest of the
/// recording session only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct SessionCorrection {
    pub misspelling: String,
    pub term: String,
}

/// Derive session terms from participant names plus free text (event title,
/// invitation body). Names contribute every name token; free text only
/// contributes visually distinctive tokens (CamelCase, acronyms), because
/// injecting ordinary words would invite corrections toward them.
pub fn derive_session_terms(names: &[String], free_texts: &[&str]) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut terms = Vec::new();
    let mut push = |token: &str| {
        if terms.len() < MAX_SESSION_TERMS && seen.insert(token.to_lowercase()) {
            terms.push(token.to_string());
        }
    };

    for token in names.iter().flat_map(|name| name.split_whitespace()) {
        let token = token.trim_matches(|c: char| !c.is_alphanumeric());
        if is_name_token(token) {
            push(token);
        }
    }

    for token in free_texts.iter().flat_map(|text| text.split_whitespace()) {
        let token = token.trim_matches(|c: char| !c.is_alphanumeric());
        if is_distinctive_token(token) {
            push(token);
        }
    }

    terms
}

/// Word-level alignment between the paragraph text before and after a live
/// edit. Each changed word pair becomes a session correction so later STT
/// output of the same misspelling is rewritten toward the user's fix.
pub fn derive_corrections_from_edit(original: &str, corrected: &str) -> Vec<SessionCorrection> {
    let orig_words = tokenize_words(original);
    let corr_words = tokenize_words(corrected);
    if orig_words.is_empty() || corr_words.is_empty() || orig_words == corr_words {
        return Vec::new();
    }

    let mut corrections = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut i = 0usize;
    let mut j = 0usize;

    while i < orig_words.len() && j < corr_words.len() {
        if orig_words[i].eq_ignore_ascii_case(&corr_words[j]) {
            i += 1;
            j += 1;
            continue;
        }

        // Skip ahead on whichever side still has a matching token later in
        // the other list (handles insertions/deletions without pairing noise).
        if j + 1 < corr_words.len()
            && orig_words[i].eq_ignore_ascii_case(&corr_words[j + 1])
        {
            j += 1;
            continue;
        }
        if i + 1 < orig_words.len()
            && orig_words[i + 1].eq_ignore_ascii_case(&corr_words[j])
        {
            i += 1;
            continue;
        }

        push_correction(&mut corrections, &mut seen, &orig_words[i], &corr_words[j]);
        i += 1;
        j += 1;
    }

    corrections
}

fn tokenize_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|token| token.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|token| !token.is_empty())
        .collect()
}

fn push_correction(
    corrections: &mut Vec<SessionCorrection>,
    seen: &mut HashSet<(String, String)>,
    from: &str,
    to: &str,
) {
    if !is_correction_candidate(from, to) {
        return;
    }
    let key = (from.to_lowercase(), to.to_lowercase());
    if seen.insert(key) {
        corrections.push(SessionCorrection {
            misspelling: from.to_string(),
            term: to.to_string(),
        });
    }
}

fn is_correction_candidate(from: &str, to: &str) -> bool {
    from.chars().count() >= MIN_TOKEN_LEN
        && to.chars().count() >= MIN_TOKEN_LEN
        && !from.eq_ignore_ascii_case(to)
}

fn is_name_token(token: &str) -> bool {
    token.chars().count() >= MIN_TOKEN_LEN
        && token
            .chars()
            .all(|c| c.is_alphabetic() || c == '-' || c == '\'')
}

/// Jargon detector for free text: CamelCase ("FluidNC") and acronyms
/// ("API"). Ordinary capitalized words (sentence starts, title case) don't
/// qualify. Digit-bearing tokens are skipped: session terms match through
/// Soundex only, and digits carry no phonetic signal.
fn is_distinctive_token(token: &str) -> bool {
    if token.chars().count() < MIN_TOKEN_LEN || !token.chars().all(char::is_alphanumeric) {
        return false;
    }
    if token.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    token.chars().skip(1).any(char::is_uppercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(terms: &[String]) -> Vec<&str> {
        terms.iter().map(String::as_str).collect()
    }

    #[test]
    fn name_tokens_are_split_and_deduped() {
        let names = vec!["Alice Martin".to_string(), "alice Dupont".to_string()];
        let terms = derive_session_terms(&names, &[]);
        assert_eq!(strs(&terms), vec!["Alice", "Martin", "Dupont"]);
    }

    #[test]
    fn short_name_tokens_are_dropped() {
        let names = vec!["Al Pacino".to_string()];
        assert_eq!(strs(&derive_session_terms(&names, &[])), vec!["Pacino"]);
    }

    #[test]
    fn free_text_keeps_only_distinctive_tokens() {
        let terms = derive_session_terms(
            &[],
            &["Sprint planning for FluidNC and the API, V6 rollout tomorrow"],
        );
        assert_eq!(strs(&terms), vec!["FluidNC", "API"]);
    }

    #[test]
    fn punctuation_is_trimmed() {
        let terms = derive_session_terms(&[], &["(FluidNC), API."]);
        assert_eq!(strs(&terms), vec!["FluidNC", "API"]);
    }

    #[test]
    fn derive_corrections_pairs_changed_words() {
        let corrections = derive_corrections_from_edit(
            "We use Kubernetis for deploys",
            "We use Kubernetes for deploys",
        );
        assert_eq!(corrections.len(), 1);
        assert_eq!(corrections[0].misspelling, "Kubernetis");
        assert_eq!(corrections[0].term, "Kubernetes");
    }

    #[test]
    fn derive_corrections_skips_identical_and_short_tokens() {
        assert!(derive_corrections_from_edit("hello world", "hello world").is_empty());
        assert!(derive_corrections_from_edit("ok fine", "ok ok").is_empty());
    }

    #[test]
    fn derive_corrections_dedupes_repeated_pairs() {
        let corrections = derive_corrections_from_edit(
            "Kubernetis and Kubernetis again",
            "Kubernetes and Kubernetes again",
        );
        assert_eq!(corrections.len(), 1);
    }

    #[test]
    fn capped_at_max_terms() {
        let text = (0..70u8)
            .map(|i| {
                format!(
                    "term{}{}X",
                    char::from(b'a' + i / 26),
                    char::from(b'a' + i % 26)
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        let terms = derive_session_terms(&[], &[text.as_str()]);
        assert_eq!(terms.len(), MAX_SESSION_TERMS);
    }
}
