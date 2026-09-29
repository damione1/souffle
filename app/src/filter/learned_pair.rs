//! Guards on machine-learned misspelling→term pairs.
//!
//! User-typed pronunciations may rewrite a stopword. A pair inferred from an
//! edit may not: it has to look like a recognition error (phonetically close)
//! and must not start from a protected function word.

use strsim::normalized_levenshtein;

/// Cap on pairs learned from one edit. Beyond this the edit is a rewrite,
/// not a handful of ASR fixes, and nothing is learned from it.
pub(crate) const MAX_LEARNED_PAIRS: usize = 8;

const MIN_TOKEN_LEN: usize = 3;

/// Same floor as derived phonetic matches in the dictionary filter.
pub(crate) const PHONETIC_SIMILARITY_FLOOR: f64 = 0.65;

/// High-frequency French and English function words that a *derived*
/// phonetic match must never replace. These are exactly the words most
/// likely to collide with a name added to the dictionary purely by
/// coincidence (short, common, phonetically generic). An explicit
/// user-typed pronunciation is the only thing allowed to override this list.
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
    "rien", "ici", // English
    "the", "then", "than", "this", "that", "these", "those", "with", "from", "have", "been",
    "were", "when", "what", "which", "who", "whom", "where", "there", "their", "they", "them",
    "will", "would", "could", "should", "just", "very", "also", "into", "onto", "over", "under",
    "about", "after", "before",
];

/// Removes accents from common French characters, folding them to their ASCII equivalents.
fn fold_french_diacritics(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'à' | 'â' | 'ä' => 'a',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            _ => c,
        })
        .collect()
}

/// Checks whether a lowercase word is a protected stopword (optionally with accents).
pub(crate) fn is_protected_stopword(word_lower: &str) -> bool {
    let folded = fold_french_diacritics(word_lower);
    PROTECTED_STOPWORDS.contains(&folded.as_str())
}

/// Whether a misspelling→term pair is close enough to an ASR error to keep.
pub(crate) fn is_learned_pair_acceptable(misspelling: &str, term: &str) -> bool {
    let miss_len = misspelling.chars().count();
    let term_len = term.chars().count();
    if miss_len < MIN_TOKEN_LEN || term_len < MIN_TOKEN_LEN {
        return false;
    }
    if misspelling.eq_ignore_ascii_case(term) {
        return false;
    }
    let miss_lower = misspelling.to_lowercase();
    if is_protected_stopword(&miss_lower) {
        return false;
    }
    let similarity = normalized_levenshtein(&miss_lower, &term.to_lowercase());
    similarity >= PHONETIC_SIMILARITY_FLOOR
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_stopword_source() {
        assert!(!is_learned_pair_acceptable("pour", "Pierre"));
        assert!(!is_learned_pair_acceptable("par", "Pierre"));
        assert!(!is_learned_pair_acceptable("être", "other"));
    }

    #[test]
    fn rejects_an_unrelated_substitution() {
        assert!(!is_learned_pair_acceptable("pense", "crois"));
    }

    #[test]
    fn keeps_a_close_asr_misspelling() {
        assert!(is_learned_pair_acceptable("Kubernetis", "Kubernetes"));
    }
}
