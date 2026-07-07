//! Session-scoped transcription hints derived from meeting context
//! (calendar participants, event title and description). These terms feed
//! the dictionary filter for one recording session only and are never
//! persisted to the user's dictionary.

use std::collections::HashSet;

/// Upper bound on derived terms: invitation bodies can be huge
/// (videoconference boilerplate), and every term costs a comparison per
/// transcribed word.
const MAX_SESSION_TERMS: usize = 50;

/// Minimum token length; shorter tokens collide with function words.
const MIN_TOKEN_LEN: usize = 3;

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

fn is_name_token(token: &str) -> bool {
    token.chars().count() >= MIN_TOKEN_LEN
        && token.chars().all(|c| c.is_alphabetic() || c == '-' || c == '\'')
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
