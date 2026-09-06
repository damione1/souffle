use crate::engine::TranscriptionSegment;
use crate::lid::LanguageCode;
use crate::settings::MeetingTranscriptionLanguage;

/// Language the summary must be written in. Same two codes the meeting
/// language setting and segment `language` fields already use.
pub use crate::lid::LanguageCode as SummaryLanguage;

/// English display name used in the system-prompt instruction. The rest of
/// the prompt stays English: a 7B follows the instruction language, so the
/// language *name* has to be explicit.
fn language_english_name(language: SummaryLanguage) -> &'static str {
    match language {
        LanguageCode::En => "English",
        LanguageCode::Fr => "French",
    }
}

/// Parse a BCP-47-ish tag (`fr`, `fr-FR`, `en_US`). Null, empty, `unknown`,
/// and anything else we do not summarize in are ignored.
fn language_from_tag(raw: &str) -> Option<SummaryLanguage> {
    let primary = raw
        .trim()
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match primary.as_str() {
        "en" => Some(LanguageCode::En),
        "fr" => Some(LanguageCode::Fr),
        _ => None,
    }
}

/// Majority vote over segment `language` values. Ties and "no identified
/// languages" return `None` so the caller can fall through to locale / English.
pub fn majority_language_from_segments(
    segments: &[TranscriptionSegment],
) -> Option<SummaryLanguage> {
    let mut en = 0u32;
    let mut fr = 0u32;
    for segment in segments {
        match segment.language.as_deref().and_then(language_from_tag) {
            Some(LanguageCode::En) => en += 1,
            Some(LanguageCode::Fr) => fr += 1,
            None => {}
        }
    }
    match (en, fr) {
        (0, 0) => None,
        (e, f) if f > e => Some(LanguageCode::Fr),
        (e, f) if e > f => Some(LanguageCode::En),
        _ => None,
    }
}

/// Resolve the language a meeting summary should be written in.
///
/// Explicit `MeetingTranscriptionLanguage` (En / Fr) wins: the user already
/// told the engine that prior via `set_meeting_language_prior`. Auto votes
/// on identified segments, then the UI locale, then English.
pub fn resolve_summary_language(
    setting: MeetingTranscriptionLanguage,
    segments: &[TranscriptionSegment],
    ui_locale: &str,
) -> SummaryLanguage {
    LanguageCode::from_setting(setting)
        .or_else(|| majority_language_from_segments(segments))
        .or_else(|| language_from_tag(ui_locale))
        .unwrap_or(LanguageCode::En)
}

/// Append an explicit output-language rule. "Same language as the
/// transcript" is not enough: a 7B follows the English system prompt.
///
/// Covers headings too, not just bullets: nothing downstream matches a
/// heading by its literal text, and an English heading over French bullets
/// reads as a bug.
pub fn with_language_instruction(system_prompt: &str, language: SummaryLanguage) -> String {
    format!(
        "{}\n\nWrite all output, including headings and bullets, in {}.",
        system_prompt.trim_end(),
        language_english_name(language)
    )
}

#[cfg(test)]
mod tests {
    use super::{
        majority_language_from_segments, resolve_summary_language, with_language_instruction,
    };
    use crate::constants::{OLLAMA_MAP_PROMPT, OLLAMA_MERGE_PROMPT, OLLAMA_SUMMARIZE_PROMPT};
    use crate::engine::TranscriptionSegment;
    use crate::lid::LanguageCode;
    use crate::settings::MeetingTranscriptionLanguage;

    fn seg(language: Option<&str>) -> TranscriptionSegment {
        TranscriptionSegment {
            text: "x".into(),
            start_time: 0.0,
            end_time: 1.0,
            is_final: true,
            language: language.map(str::to_string),
            confidence: None,
            speaker: None,
        }
    }

    #[test]
    fn majority_mixed_french_wins() {
        let segments: Vec<_> = std::iter::repeat_with(|| seg(Some("fr")))
            .take(20)
            .chain(std::iter::repeat_with(|| seg(Some("en"))).take(2))
            .collect();
        assert_eq!(
            majority_language_from_segments(&segments),
            Some(LanguageCode::Fr)
        );
    }

    #[test]
    fn majority_mixed_english_wins() {
        let segments = [seg(Some("en")), seg(Some("en")), seg(Some("fr"))];
        assert_eq!(
            majority_language_from_segments(&segments),
            Some(LanguageCode::En)
        );
    }

    #[test]
    fn majority_tie_is_undecided() {
        let segments = [seg(Some("fr")), seg(Some("en"))];
        assert_eq!(majority_language_from_segments(&segments), None);
    }

    #[test]
    fn majority_all_null_is_undecided() {
        let segments = [seg(None), seg(None), seg(Some("")), seg(Some("unknown"))];
        assert_eq!(majority_language_from_segments(&segments), None);
    }

    #[test]
    fn majority_ignores_null_unknown_and_normalizes_tags() {
        let segments = [
            seg(None),
            seg(Some("unknown")),
            seg(Some("und")),
            seg(Some("fr-FR")),
            seg(Some("FR")),
            seg(Some("en_US")),
        ];
        assert_eq!(
            majority_language_from_segments(&segments),
            Some(LanguageCode::Fr)
        );
    }

    #[test]
    fn explicit_setting_beats_segment_majority() {
        let french_meeting = [seg(Some("fr")), seg(Some("fr")), seg(Some("en"))];
        assert_eq!(
            resolve_summary_language(MeetingTranscriptionLanguage::En, &french_meeting, "fr"),
            LanguageCode::En
        );
        assert_eq!(
            resolve_summary_language(
                MeetingTranscriptionLanguage::Fr,
                &[seg(Some("en")), seg(Some("en"))],
                "en"
            ),
            LanguageCode::Fr
        );
    }

    #[test]
    fn auto_uses_majority_then_locale_then_english() {
        let french = [seg(Some("fr")), seg(Some("fr")), seg(Some("en"))];
        assert_eq!(
            resolve_summary_language(MeetingTranscriptionLanguage::Auto, &french, "en"),
            LanguageCode::Fr
        );

        let unidentified = [seg(None), seg(Some("unknown"))];
        assert_eq!(
            resolve_summary_language(MeetingTranscriptionLanguage::Auto, &unidentified, "fr-CA"),
            LanguageCode::Fr
        );
        assert_eq!(
            resolve_summary_language(MeetingTranscriptionLanguage::Auto, &unidentified, ""),
            LanguageCode::En
        );

        let tied = [seg(Some("fr")), seg(Some("en"))];
        assert_eq!(
            resolve_summary_language(MeetingTranscriptionLanguage::Auto, &tied, "fr"),
            LanguageCode::Fr
        );
        assert_eq!(
            resolve_summary_language(MeetingTranscriptionLanguage::Auto, &tied, "en"),
            LanguageCode::En
        );
    }

    #[test]
    fn map_and_final_prompts_contain_the_chosen_language() {
        let language = resolve_summary_language(
            MeetingTranscriptionLanguage::Auto,
            &[seg(Some("fr")), seg(Some("fr")), seg(Some("en"))],
            "en",
        );
        assert_eq!(language, LanguageCode::Fr);

        let map = with_language_instruction(OLLAMA_MAP_PROMPT, language);
        let merge = with_language_instruction(OLLAMA_MERGE_PROMPT, language);
        let final_pass = with_language_instruction(OLLAMA_SUMMARIZE_PROMPT, language);
        let rule = "Write all output, including headings and bullets, in French.";
        assert!(map.contains(rule));
        assert!(merge.contains(rule));
        assert!(final_pass.contains(rule));
        assert!(map.contains("same language as the transcript"));
        assert!(final_pass.contains("same language as the input"));
    }
}
