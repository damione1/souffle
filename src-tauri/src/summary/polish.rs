use serde::{Deserialize, Serialize};

use crate::filter::{DictionaryEntry, pronunciation_aliases};
use crate::settings::{AppSettings, DictationPolishTemplate};

use super::{
    SummarizeProgress, SummaryProviderKind, extract::extract_json_payload, generate_with_provider,
    pick_summary_model, resolve_provider,
};

pub const TEMPLATE_CLEAN: &str = "clean";
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
            id: TEMPLATE_CLEAN.to_string(),
            label: "Clean up".to_string(),
            prompt: "Clean this dictation. Repair words the recognizer misheard, using \
                      the surrounding sentence to tell what was meant. Discard \
                      self-corrections (\"non attends\", \"no wait\", \"scratch that\" and \
                      similar: drop the old bit, keep what follows). Honor spoken commands \
                      (new line, period, comma). Restore conventional spelling of technical \
                      terms, proper nouns, and anglicisms. Preserve the original language \
                      (French or English). Never add content that was not dictated."
                .to_string(),
        },
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

/// Built-in prompt texts shipped by earlier versions. A stored template whose
/// prompt still matches one of these was never edited by the user, so an
/// upgrade can replace it with the current default instead of pinning the old
/// wording forever (built-in ids are always present in the stored list once
/// settings have been saved once).
const SUPERSEDED_CLEAN_PROMPTS: &[&str] = &[
    "Clean this dictation without rewriting it. Discard self-corrections \
     (\"non attends\", \"no wait\", \"scratch that\" and similar: drop the old bit, keep \
     what follows). Honor spoken commands (new line, period, comma). Restore \
     conventional spelling of technical terms, proper nouns, and anglicisms. Preserve \
     the original language (French or English). Never add content that was not dictated.",
];

fn superseded_default_prompts(id: &str) -> &'static [&'static str] {
    match id {
        TEMPLATE_CLEAN => SUPERSEDED_CLEAN_PROMPTS,
        _ => &[],
    }
}

/// Merge persisted templates with defaults so new built-ins appear after upgrades
/// while keeping user-edited prompts for known ids. A stored prompt that still
/// matches a superseded built-in is treated as unedited and upgraded.
pub fn merge_polish_templates(
    stored: Vec<DictationPolishTemplate>,
) -> Vec<DictationPolishTemplate> {
    let defaults = default_polish_templates();
    if stored.is_empty() {
        return defaults;
    }

    let mut merged = Vec::with_capacity(defaults.len());
    for default in defaults {
        match stored.iter().find(|t| t.id == default.id) {
            Some(existing)
                if !superseded_default_prompts(&default.id).contains(&existing.prompt.trim()) =>
            {
                merged.push(existing.clone())
            }
            _ => merged.push(default),
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
                '\u{00ad}'
                    | '\u{034f}'
                    | '\u{061c}'
                    | '\u{115f}'
                    | '\u{1160}'
                    | '\u{17b4}'
                    | '\u{17b5}'
                    | '\u{180e}'
                    | '\u{200b}'
                    | '\u{200c}'
                    | '\u{200d}'
                    | '\u{200e}'
                    | '\u{200f}'
                    | '\u{2060}'
                    | '\u{2061}'
                    | '\u{2062}'
                    | '\u{2063}'
                    | '\u{2064}'
                    | '\u{206a}'
                    | '\u{206b}'
                    | '\u{206c}'
                    | '\u{206d}'
                    | '\u{206e}'
                    | '\u{206f}'
                    | '\u{feff}'
                    | '\u{fff9}'
                    | '\u{fffa}'
                    | '\u{fffb}'
            )
        })
        .collect()
}

pub fn is_blank_for_polish(text: &str) -> bool {
    strip_invisible_chars(text).trim().is_empty()
}

pub fn parse_polish_response(raw: &str) -> Result<String, String> {
    let plain = normalize_plain_polish(raw);
    if let Some(text) = json_text_field(&plain) {
        return require_nonempty_polish(text);
    }
    if let Some(text) = json_text_field(extract_json_payload(raw)) {
        return require_nonempty_polish(text);
    }
    require_nonempty_polish(plain)
}

fn require_nonempty_polish(text: String) -> Result<String, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        Err("Dictation polish returned empty text".into())
    } else {
        Ok(text)
    }
}

fn json_text_field(payload: &str) -> Option<String> {
    let payload = payload.trim();
    if !payload.starts_with('{') {
        return None;
    }
    let wire: PolishWire = serde_json::from_str(payload).ok()?;
    Some(wire.text.trim().to_string())
}

fn normalize_plain_polish(raw: &str) -> String {
    let mut text = raw.trim().to_string();
    text = strip_markdown_fence(&text).trim().to_string();
    text = strip_chatty_prefix(&text).trim().to_string();
    text = strip_markdown_fence(&text).trim().to_string();
    strip_wrapping_quotes(&text).trim().to_string()
}

fn strip_markdown_fence(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let rest = rest
        .strip_prefix("json")
        .or_else(|| rest.strip_prefix("JSON"))
        .unwrap_or(rest);
    let body = if let Some((_, after_lang)) = rest.split_once('\n') {
        after_lang
    } else {
        rest
    };
    if let Some(end) = body.rfind("```") {
        body[..end].trim()
    } else {
        body.trim()
    }
}

fn strip_wrapping_quotes(text: &str) -> &str {
    let trimmed = text.trim();
    let mut chars = trimmed.chars();
    let (Some(first), Some(last)) = (chars.next(), chars.next_back()) else {
        return trimmed;
    };
    let quoted = matches!(
        (first, last),
        ('"', '"') | ('\'', '\'') | ('“', '”') | ('‘', '’')
    );
    if quoted {
        &trimmed[first.len_utf8()..trimmed.len() - last.len_utf8()]
    } else {
        trimmed
    }
}

fn strip_chatty_prefix(text: &str) -> &str {
    let trimmed = text.trim();
    let Some((first, rest)) = trimmed.split_once('\n') else {
        return trimmed;
    };
    let rest = rest.trim();
    if rest.is_empty() || !is_chatty_preamble(first.trim()) {
        trimmed
    } else {
        rest
    }
}

fn is_chatty_preamble(line: &str) -> bool {
    if line.chars().count() > 80 {
        return false;
    }
    let stripped = line
        .trim()
        .trim_end_matches(['!', '.', ':', ' '])
        .to_ascii_lowercase();
    matches!(
        stripped.as_str(),
        "sure" | "ok" | "okay" | "here you go" | "of course" | "certainly" | "absolutely"
    ) || stripped.starts_with("here is the")
        || stripped.starts_with("here's the")
        || stripped.starts_with("cleaned text")
        || stripped.starts_with("cleaned dictation")
}

pub fn build_polish_user_prompt(
    template_prompt: &str,
    transcript: &str,
    dictionary: &[DictionaryEntry],
    focused_app: Option<&str>,
    rewrite_of: Option<&str>,
) -> String {
    let mut prompt = format!(
        "Instructions:\n{}\n\nDictation transcript:\n---\n{}\n---",
        template_prompt.trim(),
        transcript.trim()
    );
    if let Some(vocab) = format_dictionary_vocabulary(dictionary) {
        prompt.push_str("\n\n");
        prompt.push_str(&vocab);
    }
    if let Some(name) = focused_app.map(str::trim).filter(|name| !name.is_empty()) {
        prompt.push_str("\n\n");
        prompt.push_str("Target app: ");
        prompt.push_str(name);
        prompt.push_str(
            "\nMatch that app's usual tone and formatting conventions. Do not mention the app name in the output.",
        );
    }
    if let Some(selection) = rewrite_of.filter(|selection| !selection.trim().is_empty()) {
        prompt.push_str("\n\n");
        prompt.push_str("Rewrite this selected text (replace it; keep meaning unless the dictation changes it):\n---\n");
        prompt.push_str(selection);
        prompt.push_str("\n---\nThe dictation transcript is the user's spoken rewrite instructions. Output only the rewritten selection.");
    }
    prompt
}

fn format_dictionary_vocabulary(entries: &[DictionaryEntry]) -> Option<String> {
    let mut lines = Vec::new();
    for entry in entries {
        let term = entry.term.trim();
        if term.is_empty() {
            continue;
        }
        let aliases = pronunciation_aliases(term, entry.pronunciation.as_deref());
        if aliases.is_empty() {
            lines.push(format!("- {term}"));
        } else {
            lines.push(format!("- {term} (also heard as: {})", aliases.join(", ")));
        }
    }
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "Preferred spellings / vocabulary:\n{}",
        lines.join("\n")
    ))
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
    dictionary: &[DictionaryEntry],
    focused_app: Option<&str>,
    rewrite_of: Option<&str>,
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

    let prompt = build_polish_user_prompt(
        &template_prompt,
        &stripped,
        dictionary,
        focused_app,
        rewrite_of,
    );
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
        false,
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
            text: super::formatters::apply_post_polish_formatters(&text),
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
        SUPERSEDED_CLEAN_PROMPTS, TEMPLATE_BULLETS, TEMPLATE_CLEAN, TEMPLATE_EMAIL,
        TEMPLATE_NO_FILLERS, build_polish_user_prompt, default_polish_templates,
        early_polish_dictation_result, effective_template_prompt, is_blank_for_polish,
        merge_polish_templates, parse_polish_response, strip_invisible_chars,
        superseded_default_prompts,
    };
    use crate::filter::DictionaryEntry;
    use crate::settings::{AppSettings, DictationPolishTemplate};

    fn dict_entry(term: &str, pronunciation: Option<&str>) -> DictionaryEntry {
        DictionaryEntry {
            id: 0,
            term: term.to_string(),
            pronunciation: pronunciation.map(str::to_string),
            category: None,
            created_at: String::new(),
        }
    }

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
    fn parse_polish_response_accepts_plain_text() {
        assert_eq!(
            parse_polish_response("Hello, cleaned dictation.").unwrap(),
            "Hello, cleaned dictation."
        );
    }

    #[test]
    fn parse_polish_response_strips_wrapping_quotes() {
        assert_eq!(
            parse_polish_response("\"Hello world\"").unwrap(),
            "Hello world"
        );
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
            parse_polish_response("Sure!\n```json\n{\"text\":\"  Done  \"}\n```").unwrap(),
            "Done"
        );
        assert_eq!(
            parse_polish_response("Sure!\nHello world").unwrap(),
            "Hello world"
        );
        assert_eq!(
            parse_polish_response("Subject:\nMeeting tomorrow").unwrap(),
            "Subject:\nMeeting tomorrow"
        );
    }

    #[test]
    fn parse_polish_response_rejects_empty_text_field() {
        assert!(parse_polish_response(r#"{"text":"   "}"#).is_err());
        assert!(parse_polish_response("   ").is_err());
    }

    #[test]
    fn default_templates_include_shipped_ids() {
        let templates = default_polish_templates();
        let ids: Vec<_> = templates.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                TEMPLATE_CLEAN,
                TEMPLATE_EMAIL,
                TEMPLATE_BULLETS,
                TEMPLATE_NO_FILLERS
            ]
        );
    }

    #[test]
    fn merge_polish_templates_upgrades_an_unedited_superseded_builtin() {
        let stored = vec![DictationPolishTemplate {
            id: TEMPLATE_CLEAN.to_string(),
            label: "Clean up".to_string(),
            prompt: SUPERSEDED_CLEAN_PROMPTS[0].to_string(),
        }];
        let merged = merge_polish_templates(stored);
        let current = default_polish_templates();
        assert_eq!(merged[0].prompt, current[0].prompt);
        assert!(
            merged[0].prompt.contains("misheard"),
            "the upgraded prompt must carry the repair instruction"
        );
    }

    #[test]
    fn merge_polish_templates_keeps_an_edited_clean_template() {
        let stored = vec![DictationPolishTemplate {
            id: TEMPLATE_CLEAN.to_string(),
            label: "Clean up".to_string(),
            prompt: "My own cleanup rules".to_string(),
        }];
        let merged = merge_polish_templates(stored);
        assert_eq!(merged[0].prompt, "My own cleanup rules");
    }

    #[test]
    fn no_current_default_is_listed_as_superseded() {
        for template in default_polish_templates() {
            assert!(
                !superseded_default_prompts(&template.id).contains(&template.prompt.as_str()),
                "{} lists its current prompt as superseded, so merge would churn forever",
                template.id
            );
        }
    }

    #[test]
    fn merge_polish_templates_preserves_edits_and_adds_new_defaults() {
        let stored = vec![DictationPolishTemplate {
            id: TEMPLATE_EMAIL.to_string(),
            label: "Custom".to_string(),
            prompt: "My email prompt".to_string(),
        }];
        let merged = merge_polish_templates(stored);
        assert_eq!(merged.len(), 4);
        assert_eq!(merged[0].id, TEMPLATE_CLEAN);
        assert_eq!(merged[1].prompt, "My email prompt");
        assert_eq!(merged[2].id, TEMPLATE_BULLETS);
    }

    #[test]
    fn merge_polish_templates_inserts_clean_for_existing_three_template_users() {
        let stored = vec![
            DictationPolishTemplate {
                id: TEMPLATE_EMAIL.to_string(),
                label: "Professional email".to_string(),
                prompt: "Edited email".to_string(),
            },
            DictationPolishTemplate {
                id: TEMPLATE_BULLETS.to_string(),
                label: "Bullet points".to_string(),
                prompt: "Edited bullets".to_string(),
            },
            DictationPolishTemplate {
                id: TEMPLATE_NO_FILLERS.to_string(),
                label: "Remove fillers".to_string(),
                prompt: "Edited fillers".to_string(),
            },
        ];
        let merged = merge_polish_templates(stored);
        let ids: Vec<_> = merged.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                TEMPLATE_CLEAN,
                TEMPLATE_EMAIL,
                TEMPLATE_BULLETS,
                TEMPLATE_NO_FILLERS
            ]
        );
        assert_eq!(merged[1].prompt, "Edited email");
        assert_eq!(merged[2].prompt, "Edited bullets");
        assert_eq!(merged[3].prompt, "Edited fillers");
    }

    #[test]
    fn build_polish_user_prompt_includes_template_and_transcript() {
        let prompt = build_polish_user_prompt("Make bullets", "hello world", &[], None, None);
        assert!(prompt.contains("Make bullets"));
        assert!(prompt.contains("hello world"));
        assert!(!prompt.contains("Preferred spellings"));
        assert!(!prompt.contains("Target app:"));
        assert!(!prompt.contains("Rewrite this selected text"));
    }

    #[test]
    fn build_polish_user_prompt_includes_dictionary_aliases() {
        let prompt = build_polish_user_prompt(
            "Clean this",
            "le vésix arrive",
            &[
                dict_entry("V6", Some("vésix, vee six")),
                dict_entry("Kubernetes", None),
            ],
            None,
            None,
        );
        assert!(prompt.contains("Preferred spellings / vocabulary:"));
        assert!(prompt.contains("- V6 (also heard as: vésix, vee six)"));
        assert!(prompt.contains("- Kubernetes"));
        assert!(prompt.contains("le vésix arrive"));
    }

    #[test]
    fn build_polish_user_prompt_skips_empty_dictionary_section() {
        let prompt =
            build_polish_user_prompt("Clean this", "hello", &[dict_entry("  ", None)], None, None);
        assert!(!prompt.contains("Preferred spellings"));
    }

    #[test]
    fn build_polish_user_prompt_includes_focused_app() {
        let prompt = build_polish_user_prompt("Clean this", "hello", &[], Some("Mail"), None);
        assert!(prompt.contains("Target app: Mail"));
        assert!(prompt.contains(
            "Match that app's usual tone and formatting conventions. Do not mention the app name in the output."
        ));
        assert!(!prompt.contains("Rewrite this selected text"));
    }

    #[test]
    fn build_polish_user_prompt_includes_rewrite_of() {
        let prompt = build_polish_user_prompt(
            "Clean this",
            "make it shorter",
            &[],
            None,
            Some("The original paragraph."),
        );
        assert!(prompt.contains(
            "Rewrite this selected text (replace it; keep meaning unless the dictation changes it):"
        ));
        assert!(prompt.contains("---\nThe original paragraph.\n---"));
        assert!(prompt.contains(
            "The dictation transcript is the user's spoken rewrite instructions. Output only the rewritten selection."
        ));
        assert!(!prompt.contains("Target app:"));
    }

    #[test]
    fn build_polish_user_prompt_omits_blank_app_and_rewrite_context() {
        let prompt = build_polish_user_prompt("Clean this", "hello", &[], Some("  "), Some("\n"));
        assert!(!prompt.contains("Target app:"));
        assert!(!prompt.contains("Rewrite this selected text"));
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
