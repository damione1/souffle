use serde::{Deserialize, Serialize};

use crate::settings::{AppSettings, DictationPolishTemplate};

use super::{
    SummaryProviderKind, SummarizeProgress, extract::extract_json_payload, generate_with_provider,
    pick_summary_model, resolve_provider,
};

pub const TEMPLATE_EMAIL: &str = "email";
pub const TEMPLATE_BULLETS: &str = "bullets";
pub const TEMPLATE_NO_FILLERS: &str = "no_fillers";

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
pub struct DictationPolishResult {
    pub text: String,
    /// True when polish was skipped (disabled, blank input, or no provider).
    pub skipped: bool,
    /// Set when polish was attempted but failed; the returned text is the
    /// pre-polish input (after invisible-char stripping).
    pub warning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PolishWire {
    text: String,
}

/// Built-in polish templates shipped with the app. User edits are persisted
/// per-id; missing ids are filled from these defaults on load.
pub fn default_polish_templates() -> Vec<DictationPolishTemplate> {
    vec![
        DictationPolishTemplate {
            id: TEMPLATE_EMAIL.to_string(),
            label: "Professional email".to_string(),
            prompt: "Rewrite the dictation as a clear professional email. Fix grammar and \
                      punctuation. Preserve the meaning and original language."
                .to_string(),
        },
        DictationPolishTemplate {
            id: TEMPLATE_BULLETS.to_string(),
            label: "Bullet points".to_string(),
            prompt: "Convert the dictation into a concise bullet list with one idea per bullet. \
                      Preserve the original language."
                .to_string(),
        },
        DictationPolishTemplate {
            id: TEMPLATE_NO_FILLERS.to_string(),
            label: "Remove fillers".to_string(),
            prompt: "Remove filler words (um, uh, like, you know), false starts, and repeated \
                      words. Keep everything else as close to verbatim as possible. Preserve \
                      the original language."
                .to_string(),
        },
    ]
}

/// Merge persisted templates with defaults so new built-ins appear after upgrades
/// while keeping user-edited prompts for known ids.
pub fn merge_polish_templates(stored: Vec<DictationPolishTemplate>) -> Vec<DictationPolishTemplate> {
    let defaults = default_polish_templates();
    if stored.is_empty() {
        return defaults;
    }

    let mut merged = Vec::with_capacity(defaults.len());
    for default in defaults {
        if let Some(existing) = stored.iter().find(|t| t.id == default.id) {
            merged.push(existing.clone());
        } else {
            merged.push(default);
        }
    }
    merged
}

pub fn resolve_active_template(settings: &AppSettings) -> Option<&DictationPolishTemplate> {
    settings
        .dictation_polish_templates
        .iter()
        .find(|template| template.id == settings.dictation_polish_template_id)
        .or_else(|| settings.dictation_polish_templates.first())
}

/// Returns immediately when polish is disabled or the stripped input is blank.
/// Callers can skip provider probing when this returns `Some`.
pub fn early_polish_dictation_result(
    settings: &AppSettings,
    raw_text: &str,
) -> Option<DictationPolishResult> {
    let stripped = strip_invisible_chars(raw_text);

    if !settings.dictation_polish_enabled {
        return Some(DictationPolishResult {
            text: stripped.trim().to_string(),
            skipped: true,
            warning: None,
        });
    }

    if is_blank_for_polish(&stripped) {
        return Some(DictationPolishResult {
            text: String::new(),
            skipped: true,
            warning: None,
        });
    }

    None
}

/// User-edited template prompts fall back to shipped defaults when cleared.
pub fn effective_template_prompt(template: &DictationPolishTemplate) -> Result<String, String> {
    let trimmed = template.prompt.trim();
    if !trimmed.is_empty() {
        return Ok(trimmed.to_string());
    }

    if let Some(default) = default_polish_templates()
        .iter()
        .find(|candidate| candidate.id == template.id)
    {
        let fallback = default.prompt.trim();
        if !fallback.is_empty() {
            return Ok(fallback.to_string());
        }
    }

    Err("Dictation polish prompt is empty".into())
}

/// Strip zero-width and other invisible characters that often leak from STT
/// engines or paste targets, while keeping newlines and tabs.
pub fn strip_invisible_chars(text: &str) -> String {
    text.chars()
        .filter(|ch| {
            if matches!(ch, '\n' | '\r' | '\t') {
                return true;
            }
            if ch.is_control() {
                return false;
            }
            !matches!(
                ch,
                '\u{00ad}' | '\u{034f}' | '\u{061c}' | '\u{115f}' | '\u{1160}' | '\u{17b4}'
                    | '\u{17b5}' | '\u{180e}' | '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{200e}'
                    | '\u{200f}' | '\u{2060}' | '\u{2061}' | '\u{2062}' | '\u{2063}' | '\u{2064}'
                    | '\u{206a}' | '\u{206b}' | '\u{206c}' | '\u{206d}' | '\u{206e}' | '\u{206f}'
                    | '\u{feff}' | '\u{fff9}' | '\u{fffa}' | '\u{fffb}'
            )
        })
        .collect()
}

pub fn is_blank_for_polish(text: &str) -> bool {
    strip_invisible_chars(text).trim().is_empty()
}

pub fn parse_polish_response(raw: &str) -> Result<String, String> {
    let payload = extract_json_payload(raw);
    let wire: PolishWire = serde_json::from_str(payload)
        .map_err(|e| format!("Parse dictation polish JSON: {e}"))?;
    let text = wire.text.trim().to_string();
    if text.is_empty() {
        return Err("Dictation polish returned empty text".into());
    }
    Ok(text)
}

pub fn build_polish_user_prompt(template_prompt: &str, transcript: &str) -> String {
    format!(
        "Instructions:\n{}\n\nDictation transcript:\n---\n{}\n---",
        template_prompt.trim(),
        transcript.trim()
    )
}

fn polish_system_prompt(provider: SummaryProviderKind) -> &'static str {
    match provider {
        SummaryProviderKind::Ollama => super::ollama::DICTATION_POLISH_SYSTEM_PROMPT,
        SummaryProviderKind::AppleIntelligence => super::apple::DICTATION_POLISH_SYSTEM_PROMPT,
    }
}

/// Apply LLM polish when enabled and a provider is available. On failure, returns
/// the stripped input with a warning so paste/history still succeed.
pub async fn polish_dictation_text(
    settings: &AppSettings,
    raw_text: &str,
    available_models: &[super::SummaryModelDescriptor],
) -> DictationPolishResult {
    let stripped = strip_invisible_chars(raw_text);

    if let Some(result) = early_polish_dictation_result(settings, raw_text) {
        return result;
    }

    let Some(template) = resolve_active_template(settings) else {
        return DictationPolishResult {
            text: stripped.trim().to_string(),
            skipped: true,
            warning: Some("No dictation polish template configured".into()),
        };
    };

    let Some(model) = pick_summary_model(settings, available_models) else {
        return DictationPolishResult {
            text: stripped.trim().to_string(),
            skipped: true,
            warning: Some(
                "No summarization provider available — install Ollama or enable Apple Intelligence"
                    .into(),
            ),
        };
    };

    let provider = match resolve_provider(&model) {
        Ok(provider) => provider,
        Err(err) => {
            return DictationPolishResult {
                text: stripped.trim().to_string(),
                skipped: true,
                warning: Some(err),
            };
        }
    };

    let template_prompt = match effective_template_prompt(template) {
        Ok(prompt) => prompt,
        Err(warning) => {
            return DictationPolishResult {
                text: stripped.trim().to_string(),
                skipped: true,
                warning: Some(warning),
            };
        }
    };

    let prompt = build_polish_user_prompt(&template_prompt, &stripped);
    let no_op = |_: SummarizeProgress| {};
    let raw = match generate_with_provider(
        provider,
        &model,
        &settings.ollama_url,
        polish_system_prompt(provider),
        prompt,
        0.1,
        super::ollama::REDUCE_CONTEXT,
        &no_op,
        provider == SummaryProviderKind::Ollama,
    )
    .await
    {
        Ok(raw) => raw,
        Err(err) => {
            return DictationPolishResult {
                text: stripped.trim().to_string(),
                skipped: false,
                warning: Some(err),
            };
        }
    };

    match parse_polish_response(&raw) {
        Ok(text) => DictationPolishResult {
            text,
            skipped: false,
            warning: None,
        },
        Err(err) => DictationPolishResult {
            text: stripped.trim().to_string(),
            skipped: false,
            warning: Some(err),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TEMPLATE_BULLETS, TEMPLATE_EMAIL, TEMPLATE_NO_FILLERS, build_polish_user_prompt,
        default_polish_templates, early_polish_dictation_result, effective_template_prompt,
        is_blank_for_polish, merge_polish_templates, parse_polish_response,
        strip_invisible_chars,
    };
    use crate::settings::{AppSettings, DictationPolishTemplate};

    #[test]
    fn strip_invisible_chars_removes_zero_width_but_keeps_newlines() {
        let input = "Hello\u{200b}world\nline\u{feff}two";
        assert_eq!(strip_invisible_chars(input), "Helloworld\nlinetwo");
    }

    #[test]
    fn strip_invisible_chars_removes_soft_hyphen() {
        assert_eq!(strip_invisible_chars("soft\u{00ad}hyphen"), "softhyphen");
    }

    #[test]
    fn blank_input_is_skipped_for_polish() {
        assert!(is_blank_for_polish(""));
        assert!(is_blank_for_polish("   \u{200b}\n  "));
        assert!(!is_blank_for_polish("hello"));
    }

    #[test]
    fn parse_polish_response_accepts_bare_json() {
        assert_eq!(
            parse_polish_response(r#"{"text":"Polished output"}"#).unwrap(),
            "Polished output"
        );
    }

    #[test]
    fn parse_polish_response_strips_fence_and_chatty_prefix() {
        assert_eq!(
            parse_polish_response(
                "Sure!\n```json\n{\"text\":\"  Done  \"}\n```"
            )
            .unwrap(),
            "Done"
        );
    }

    #[test]
    fn parse_polish_response_rejects_empty_text_field() {
        assert!(parse_polish_response(r#"{"text":"   "}"#).is_err());
    }

    #[test]
    fn default_templates_include_shipped_ids() {
        let templates = default_polish_templates();
        let ids: Vec<_> = templates.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec![TEMPLATE_EMAIL, TEMPLATE_BULLETS, TEMPLATE_NO_FILLERS]);
    }

    #[test]
    fn merge_polish_templates_preserves_edits_and_adds_new_defaults() {
        let stored = vec![DictationPolishTemplate {
            id: TEMPLATE_EMAIL.to_string(),
            label: "Custom".to_string(),
            prompt: "My email prompt".to_string(),
        }];
        let merged = merge_polish_templates(stored);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].prompt, "My email prompt");
        assert_eq!(merged[1].id, TEMPLATE_BULLETS);
    }

    #[test]
    fn build_polish_user_prompt_includes_template_and_transcript() {
        let prompt = build_polish_user_prompt("Make bullets", "hello world");
        assert!(prompt.contains("Make bullets"));
        assert!(prompt.contains("hello world"));
    }

    #[test]
    fn early_polish_dictation_skips_when_disabled_without_providers() {
        let settings = AppSettings {
            dictation_polish_enabled: false,
            ..AppSettings::default()
        };

        let result = early_polish_dictation_result(&settings, "hello world").unwrap();
        assert!(result.skipped);
        assert_eq!(result.text, "hello world");
        assert!(result.warning.is_none());
    }

    #[test]
    fn early_polish_dictation_skips_blank_without_providers() {
        let settings = AppSettings {
            dictation_polish_enabled: true,
            ..AppSettings::default()
        };

        let result = early_polish_dictation_result(&settings, "   \u{200b}\n  ").unwrap();
        assert!(result.skipped);
        assert!(result.text.is_empty());
        assert!(result.warning.is_none());
    }

    #[test]
    fn early_polish_dictation_returns_none_when_polish_would_run() {
        let settings = AppSettings {
            dictation_polish_enabled: true,
            ..AppSettings::default()
        };

        assert!(early_polish_dictation_result(&settings, "hello").is_none());
    }

    #[test]
    fn effective_template_prompt_falls_back_to_default_when_cleared() {
        let template = DictationPolishTemplate {
            id: TEMPLATE_EMAIL.to_string(),
            label: "Email".to_string(),
            prompt: "   ".to_string(),
        };

        let prompt = effective_template_prompt(&template).unwrap();
        assert_eq!(
            prompt,
            default_polish_templates()
                .into_iter()
                .find(|candidate| candidate.id == TEMPLATE_EMAIL)
                .expect("default email template")
                .prompt
        );
    }

    #[test]
    fn effective_template_prompt_rejects_empty_custom_and_default() {
        let template = DictationPolishTemplate {
            id: "custom".to_string(),
            label: "Custom".to_string(),
            prompt: "   ".to_string(),
        };

        assert!(effective_template_prompt(&template).is_err());
    }
}
