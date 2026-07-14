//! Lightweight French/English language identification for meeting segments.
//!
//! Heuristic-only: accent density plus stopword hits. Used to populate
//! `segment.language` when Kyutai leaves it unset and to drive per-lane
//! mismatch resets. Never passed to moshi as a forced language.

use crate::settings::MeetingTranscriptionLanguage;

/// BCP-47-ish codes we emit on segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageCode {
    En,
    Fr,
}

impl LanguageCode {
    pub fn as_str(self) -> &'static str {
        match self {
            LanguageCode::En => "en",
            LanguageCode::Fr => "fr",
        }
    }

    pub fn from_setting(prior: MeetingTranscriptionLanguage) -> Option<Self> {
        match prior {
            MeetingTranscriptionLanguage::Auto => None,
            MeetingTranscriptionLanguage::En => Some(LanguageCode::En),
            MeetingTranscriptionLanguage::Fr => Some(LanguageCode::Fr),
        }
    }
}

/// Score a token for English vs French cues.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct LangScore {
    en: u32,
    fr: u32,
}

impl LangScore {
    fn winner(self) -> Option<LanguageCode> {
        if self.fr > self.en {
            Some(LanguageCode::Fr)
        } else if self.en > self.fr {
            Some(LanguageCode::En)
        } else {
            None
        }
    }
}

const FR_STOPWORDS: &[&str] = &[
    "le", "la", "les", "de", "des", "du", "un", "une", "et", "est", "en", "que", "qui", "dans",
    "pour", "pas", "sur", "avec", "ce", "cette", "mon", "ma", "mes", "ton", "ta", "tes", "son",
    "sa", "ses", "nous", "vous", "ils", "elles", "je", "tu", "il", "elle", "on", "ne", "au", "aux",
    "ou", "mais", "donc", "car", "comme", "plus", "tout", "tous", "toute", "toutes", "chez", "bien",
    "très", "aussi", "être", "avoir", "faire", "dit", "peut", "sont", "été", "c'est", "qu'il",
    "qu'on", "d'un", "d'une", "l'on", "l'un", "l'une", "bonjour", "merci", "réunion", "reunion",
    "oui", "non", "alors", "voilà", "voila", "parce", "quoi", "comment", "pourquoi", "maintenant",
    "aujourd'hui", "demain", "hier", "français", "francais",
];

const EN_STOPWORDS: &[&str] = &[
    "the", "and", "is", "are", "was", "were", "have", "has", "had", "not", "but", "for", "with",
    "this", "that", "from", "they", "you", "your", "our", "their", "what", "when", "where", "who",
    "which", "will", "would", "could", "should", "about", "into", "there", "here", "been", "being",
    "does", "did", "can", "just", "also", "very", "because", "than", "then", "them", "these",
    "those", "some", "any", "all", "out", "over", "after", "before", "between", "through",
];

fn normalize_token(raw: &str) -> String {
    raw.trim()
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_ascii_lowercase()
}

fn score_token(token: &str) -> LangScore {
    let mut score = LangScore::default();
    if token.is_empty() {
        return score;
    }

    for ch in token.chars() {
        match ch {
            'é' | 'è' | 'ê' | 'ë' | 'à' | 'â' | 'ù' | 'û' | 'ô' | 'î' | 'ï' | 'ç' | 'œ' | 'æ' => {
                score.fr += 2;
            }
            _ => {}
        }
    }

    if FR_STOPWORDS.contains(&token) {
        score.fr += 3;
    }
    if EN_STOPWORDS.contains(&token) {
        score.en += 3;
    }

    if token.ends_with("ment") || token.ends_with("tion") && token.contains('é') {
        score.fr += 1;
    }
    if token.ends_with("ing") || token.ends_with("ness") || token.ends_with("tion") {
        score.en += 1;
    }

    score
}

/// Detect language for a single word or short fragment.
pub fn detect_word(text: &str) -> Option<LanguageCode> {
    let token = normalize_token(text);
    if token.len() < 2 {
        return None;
    }
    let score = score_token(&token);
    if score.fr == 0 && score.en == 0 {
        return None;
    }
    score.winner()
}

/// Detect language for a longer phrase (multiple tokens).
pub fn detect_text(text: &str) -> Option<LanguageCode> {
    let mut total = LangScore::default();
    for token in text.split_whitespace() {
        let normalized = normalize_token(token);
        if normalized.len() < 2 {
            continue;
        }
        let s = score_token(&normalized);
        total.en += s.en;
        total.fr += s.fr;
    }
    total.winner()
}

/// Consecutive words that disagree with the lane prior before forcing reset.
pub const MISMATCH_STREAK_THRESHOLD: usize = 3;

/// Rolling per-lane language state with hysteresis for mismatch resets.
#[derive(Debug, Clone)]
pub struct LanguageLaneTracker {
    setting_prior: MeetingTranscriptionLanguage,
    /// Inferred majority when setting is Auto (after enough evidence).
    inferred_prior: Option<LanguageCode>,
    en_hits: u32,
    fr_hits: u32,
    mismatch_lang: Option<LanguageCode>,
    mismatch_streak: usize,
}

impl LanguageLaneTracker {
    pub fn new(setting_prior: MeetingTranscriptionLanguage) -> Self {
        Self {
            setting_prior,
            inferred_prior: None,
            en_hits: 0,
            fr_hits: 0,
            mismatch_lang: None,
            mismatch_streak: 0,
        }
    }

    pub fn reset(&mut self) {
        self.inferred_prior = None;
        self.en_hits = 0;
        self.fr_hits = 0;
        self.mismatch_lang = None;
        self.mismatch_streak = 0;
    }

    fn effective_prior(&self) -> Option<LanguageCode> {
        LanguageCode::from_setting(self.setting_prior).or(self.inferred_prior)
    }

    fn note_majority(&mut self, detected: LanguageCode) {
        match detected {
            LanguageCode::En => self.en_hits += 1,
            LanguageCode::Fr => self.fr_hits += 1,
        }
        let total = self.en_hits + self.fr_hits;
        if total >= 5 {
            self.inferred_prior = if self.fr_hits > self.en_hits {
                Some(LanguageCode::Fr)
            } else if self.en_hits > self.fr_hits {
                Some(LanguageCode::En)
            } else {
                None
            };
        }
    }

    /// Record a word; returns true when a lane reset should be forced.
    pub fn on_word(&mut self, text: &str) -> bool {
        let Some(detected) = detect_word(text) else {
            return false;
        };

        self.note_majority(detected);

        let Some(prior) = self.effective_prior() else {
            self.mismatch_lang = None;
            self.mismatch_streak = 0;
            return false;
        };

        if detected == prior {
            self.mismatch_lang = None;
            self.mismatch_streak = 0;
            return false;
        }

        if self.mismatch_lang == Some(detected) {
            self.mismatch_streak += 1;
        } else {
            self.mismatch_lang = Some(detected);
            self.mismatch_streak = 1;
        }

        self.mismatch_streak >= MISMATCH_STREAK_THRESHOLD
    }
}

/// Per-batch language trackers (one or two lanes).
#[derive(Debug, Clone)]
pub struct LanguageTracker {
    lanes: Vec<LanguageLaneTracker>,
}

impl LanguageTracker {
    pub fn new(batch_size: usize, setting_prior: MeetingTranscriptionLanguage) -> Self {
        Self {
            lanes: (0..batch_size)
                .map(|_| LanguageLaneTracker::new(setting_prior))
                .collect(),
        }
    }

    pub fn resize(&mut self, batch_size: usize, setting_prior: MeetingTranscriptionLanguage) {
        if self.lanes.len() == batch_size {
            for lane in &mut self.lanes {
                lane.setting_prior = setting_prior;
            }
            return;
        }
        *self = Self::new(batch_size, setting_prior);
    }

    pub fn reset_lane(&mut self, batch_idx: usize) {
        if let Some(lane) = self.lanes.get_mut(batch_idx) {
            lane.reset();
        }
    }

    pub fn reset_all(&mut self) {
        for lane in &mut self.lanes {
            lane.reset();
        }
    }

    /// Returns true when this lane needs a forced reset due to language mismatch.
    pub fn on_word(&mut self, text: &str, batch_idx: usize) -> bool {
        self.lanes
            .get_mut(batch_idx)
            .is_some_and(|lane| lane.on_word(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::MeetingTranscriptionLanguage;

    #[test]
    fn detect_french_accents_and_stopwords() {
        assert_eq!(detect_word("réunion"), Some(LanguageCode::Fr));
        assert_eq!(detect_word("bonjour"), Some(LanguageCode::Fr));
        assert_eq!(detect_text("le projet est très bien"), Some(LanguageCode::Fr));
    }

    #[test]
    fn detect_english_stopwords() {
        assert_eq!(detect_word("the"), Some(LanguageCode::En));
        assert_eq!(detect_text("the meeting is about planning"), Some(LanguageCode::En));
    }

    #[test]
    fn ambiguous_short_tokens_return_none() {
        assert_eq!(detect_word("ok"), None);
        assert_eq!(detect_word("12"), None);
    }

    #[test]
    fn mismatch_streak_triggers_after_threshold() {
        let mut lane = LanguageLaneTracker::new(MeetingTranscriptionLanguage::En);
        assert!(!lane.on_word("bonjour"));
        assert!(!lane.on_word("merci"));
        assert!(lane.on_word("réunion"));
    }

    #[test]
    fn mismatch_streak_clears_on_agreement() {
        let mut lane = LanguageLaneTracker::new(MeetingTranscriptionLanguage::En);
        assert!(!lane.on_word("bonjour"));
        assert!(!lane.on_word("merci"));
        assert!(!lane.on_word("the"));
        assert!(!lane.on_word("réunion"));
        assert!(!lane.on_word("encore"));
    }

    #[test]
    fn auto_prior_inferred_from_majority() {
        let mut lane = LanguageLaneTracker::new(MeetingTranscriptionLanguage::Auto);
        for _ in 0..4 {
            assert!(!lane.on_word("the"));
        }
        for _ in 0..2 {
            assert!(!lane.on_word("bonjour"));
        }
        assert_eq!(lane.effective_prior(), Some(LanguageCode::En));
    }

}
