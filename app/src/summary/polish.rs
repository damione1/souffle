use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

use crate::filter::{DictionaryEntry, pronunciation_aliases};
use crate::settings::{AppBranchRule, AppBranchTarget, AppSettings, DictationPolishTemplate};

use super::{
    SummarizeProgress, SummaryProviderKind, choose_summary_model, extract::extract_json_payload,
    generate_with_provider, resolve_provider,
};

pub const TEMPLATE_CLEAN: &str = "clean";
pub const TEMPLATE_EMAIL: &str = "email";
pub const TEMPLATE_BULLETS: &str = "bullets";
pub const TEMPLATE_NO_FILLERS: &str = "no_fillers";
pub const TEMPLATE_CHAT: &str = "chat";
pub const TEMPLATE_CODE: &str = "code";

/// Time to bring the intended app forward after pressing Tester in Settings.
pub const APP_BRANCH_TEST_DELAY: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppBranchTestPhase {
    Idle,
    Waiting,
    Resolving,
    Result,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
                      terms, proper nouns, and anglicisms. Preserve every other complete \
                      sentence and fact, including a short opening sentence. Preserve the \
                      original language (French or English). Never summarize, omit, translate, \
                      or add content."
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
        DictationPolishTemplate {
            id: TEMPLATE_CHAT.to_string(),
            label: "Chat".to_string(),
            prompt: "Clean the dictation as a short, natural chat message. Fix grammar and \
                     punctuation, keep a conversational tone, and preserve all facts and the \
                     original language. Do not add greetings, explanations, or new content."
                .to_string(),
        },
        DictationPolishTemplate {
            id: TEMPLATE_CODE.to_string(),
            label: "Code".to_string(),
            prompt: "Clean this dictation for a code editor. Preserve identifiers, code, \
                     file paths, commands, and technical terms exactly. Fix only surrounding \
                     prose and punctuation. Preserve the meaning and original language. \
                     Do not invent code or add explanations."
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
    "Clean this dictation. Repair words the recognizer misheard, using \
     the surrounding sentence to tell what was meant. Discard \
     self-corrections (\"non attends\", \"no wait\", \"scratch that\" and \
     similar: drop the old bit, keep what follows). Honor spoken commands \
     (new line, period, comma). Restore conventional spelling of technical \
     terms, proper nouns, and anglicisms. Preserve the original language \
     (French or English). Never add content that was not dictated.",
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
    let stored: Vec<_> = stored
        .into_iter()
        .filter_map(|mut template| {
            template.id = template.id.trim().to_string();
            (!template.id.is_empty()).then_some(template)
        })
        .collect();
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
    // Template ids are an open catalogue. Preserve stored custom entries in
    // their order, with the first occurrence owning a shared id, just as for
    // the built-ins above. New presets must not discard a branch's prompt.
    let mut ids: HashSet<_> = merged.iter().map(|template| template.id.clone()).collect();
    for template in stored {
        if ids.insert(template.id.clone()) {
            merged.push(template);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppBranchFallback {
    PolishDisabled,
    AppUnavailable,
    NoMatch,
    InvalidPattern,
    MissingTemplate,
}

#[derive(Debug)]
pub enum AppBranchResolution<'a> {
    Rule {
        rule: &'a AppBranchRule,
        template: &'a DictationPolishTemplate,
    },
    Global {
        reason: AppBranchFallback,
        template: Option<&'a DictationPolishTemplate>,
    },
}

impl<'a> AppBranchResolution<'a> {
    pub fn template(&self) -> Option<&'a DictationPolishTemplate> {
        match self {
            Self::Rule { template, .. } => Some(template),
            Self::Global { template, .. } => *template,
        }
    }
}

/// Shared by settings preview and the finalization pipeline. Invalid rules
/// cannot hide a later valid match; no valid winner always preserves global.
pub fn resolve_app_branch<'a>(
    settings: &'a AppSettings,
    focused_app: Option<&str>,
) -> AppBranchResolution<'a> {
    let global = resolve_active_template(settings);
    let fallback = |reason| AppBranchResolution::Global {
        reason,
        template: global,
    };
    if !settings.dictation_polish_enabled {
        return fallback(AppBranchFallback::PolishDisabled);
    }
    let Some(app) = focused_app.map(str::trim).filter(|app| !app.is_empty()) else {
        return fallback(AppBranchFallback::AppUnavailable);
    };
    let app = app.to_lowercase();
    let mut reason = AppBranchFallback::NoMatch;
    for rule in settings
        .dictation_app_branches
        .iter()
        .filter(|rule| rule.enabled)
    {
        let pattern = rule.app_pattern.trim();
        if pattern.is_empty() {
            reason = AppBranchFallback::InvalidPattern;
            continue;
        }
        if !app.contains(&pattern.to_lowercase()) {
            continue;
        }
        let template = match &rule.target {
            AppBranchTarget::Global => global,
            AppBranchTarget::Template(id) => settings
                .dictation_polish_templates
                .iter()
                .find(|template| template.id == id.0),
        };
        if let Some(template) = template {
            return AppBranchResolution::Rule { rule, template };
        }
        reason = AppBranchFallback::MissingTemplate;
    }
    fallback(reason)
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
) -> String {
    // Context first, transcript last, nothing after. Putting "Target app:"
    // after the transcript made Apple Intelligence echo it (or keep writing).
    let mut prompt = String::from(
        "Reply with the cleaned dictation only. No labels, no markdown fences, no commentary.\n\n",
    );
    prompt.push_str("Instructions:\n");
    prompt.push_str(template_prompt.trim());
    if let Some(vocab) = format_dictionary_vocabulary(dictionary) {
        prompt.push_str("\n\n");
        prompt.push_str(&vocab);
    }
    if let Some(name) = focused_app
        .map(str::trim)
        .filter(|name| !name.is_empty() && !is_own_app_name(name))
    {
        prompt.push_str("\n\n");
        prompt.push_str("Target app: ");
        prompt.push_str(name);
        prompt.push_str(
            "\nMatch that app's usual tone and formatting conventions. Do not mention the app name in the output.",
        );
    }
    prompt.push_str("\n\nDictation transcript:\n---\n");
    prompt.push_str(transcript.trim());
    prompt.push_str("\n---");
    prompt
}

/// Localized names of this app. Matching Mail's tone is useful; matching
/// Soufflé's is not, and it is what the model echoed when polish ran from
/// the main window.
fn is_own_app_name(name: &str) -> bool {
    let folded: String = name
        .trim()
        .chars()
        .map(|c| match c {
            'é' | 'É' => 'e',
            other => other,
        })
        .collect();
    folded.eq_ignore_ascii_case("souffle")
}

const PROMPT_LEAK_MARKERS: &[&str] = &[
    "Target app:",
    "Match that app's usual tone",
    "Do not mention the app name",
    "Dictation transcript:",
    "Preferred spellings / vocabulary:",
    "Reply with the cleaned dictation only",
    "Instructions:",
];

fn find_ignore_ascii_case(hay: &str, needle: &str) -> Option<usize> {
    hay.to_ascii_lowercase().find(&needle.to_ascii_lowercase())
}

/// Drop instruction text the model copied out of the user prompt. Markers
/// that were actually dictated are left alone.
fn strip_prompt_leakage(output: &str, transcript: &str) -> String {
    let mut cut = output.len();
    for marker in PROMPT_LEAK_MARKERS {
        if find_ignore_ascii_case(transcript, marker).is_some() {
            continue;
        }
        if let Some(idx) = find_ignore_ascii_case(output, marker) {
            cut = cut.min(idx);
        }
    }
    if find_ignore_ascii_case(transcript, "---").is_none() {
        let mut offset = 0;
        for line in output.split_inclusive('\n') {
            if line.trim_end_matches(['\n', '\r']).trim() == "---" {
                cut = cut.min(offset);
                break;
            }
            offset += line.len();
        }
    }
    let mut kept = output.get(..cut).unwrap_or(output).trim().to_string();
    while let Some(stripped) = kept.strip_suffix("---") {
        kept = stripped.trim_end().to_string();
    }
    kept
}

fn word_count(text: &str) -> usize {
    text.split_whitespace()
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .count()
}

fn expanded_too_much(input: &str, output: &str) -> bool {
    let inn = word_count(input);
    let out = word_count(output);
    out > inn.saturating_add(inn / 2).saturating_add(8)
}

/// Splits text into lowercase, accent-insensitive lexical tokens.
fn normalized_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    for ch in text.nfd().filter(|ch| !is_combining_mark(*ch)) {
        for lower in ch.to_lowercase() {
            if lower.is_alphanumeric() {
                word.push(lower);
            } else if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Returns whether a normalized token is removable speech filler.
fn is_filler_word(word: &str) -> bool {
    matches!(word, "um" | "uh" | "erm" | "euh" | "hum" | "hmm" | "like")
}

/// Recognizes a sentence that opens with a command retracting prior content.
fn starts_with_self_correction(words: &[String]) -> bool {
    matches!(
        words,
        [first, second, ..]
            if matches!(
                (first.as_str(), second.as_str()),
                ("scratch", "that") | ("no", "wait") | ("non", "attends") | ("never", "mind")
            )
    )
}

/// Recognizes a standalone correction command with no replacement content.
fn is_self_correction_sentence(words: &[String]) -> bool {
    words.len() == 2 && starts_with_self_correction(words)
}

struct NormalizedSentence<'a> {
    raw: &'a str,
    words: Vec<String>,
}

/// Splits source text into non-empty normalized sentences while retaining the
/// raw slice needed to recognize apostrophe-based French negation.
fn normalized_sentences(input: &str) -> Vec<NormalizedSentence<'_>> {
    input
        .split(['.', '!', '?', '\n', '\r'])
        .filter_map(|raw| {
            let words = normalized_words(raw);
            (!words.is_empty()).then_some(NormalizedSentence { raw, words })
        })
        .collect()
}

/// Returns whether a sentence is a retraction command or the content it retracts.
fn is_retraction_or_retracted(sentences: &[NormalizedSentence<'_>], index: usize) -> bool {
    is_self_correction_sentence(&sentences[index].words)
        || sentences
            .get(index + 1)
            .is_some_and(|next| starts_with_self_correction(&next.words))
}

/// Removes an inline correction command while retaining its replacement text.
fn required_sentence_words(words: &[String]) -> &[String] {
    if words.len() > 2 && starts_with_self_correction(words) {
        &words[2..]
    } else {
        words
    }
}

/// Detects a source sentence whose substantive or distinctive content vanished.
fn dropped_substantive_sentence(
    input: &str,
    output_words: &HashSet<String>,
    allow_filler_removal: bool,
) -> bool {
    let sentences = normalized_sentences(input);

    sentences.iter().enumerate().any(|(index, sentence)| {
        if is_retraction_or_retracted(&sentences, index) {
            return false;
        }

        let words: Vec<&str> = required_sentence_words(&sentence.words)
            .iter()
            .map(String::as_str)
            .filter(|word| !allow_filler_removal || !is_filler_word(word))
            .collect();
        if words.len() < 2 {
            return false;
        }

        let retained = words
            .iter()
            .filter(|word| output_words.contains(**word))
            .count();
        if retained.saturating_mul(100) < words.len().saturating_mul(30) {
            return true;
        }

        let distinctive: HashSet<&str> = words
            .iter()
            .copied()
            .filter(|word| {
                !sentences.iter().enumerate().any(|(other_index, other)| {
                    other_index != index
                        && !is_retraction_or_retracted(&sentences, other_index)
                        && required_sentence_words(&other.words)
                            .iter()
                            .any(|candidate| candidate == word)
                })
            })
            .collect();
        let retained_distinctive = distinctive
            .iter()
            .filter(|word| output_words.contains(**word))
            .count();
        distinctive.len() >= 2 && retained_distinctive < 2
    })
}

/// Returns source words that the selected template is required to retain.
fn retained_input_words(input: &str, allow_filler_removal: bool) -> Vec<String> {
    let sentences = normalized_sentences(input);
    let mut retained = Vec::new();
    for (index, sentence) in sentences.iter().enumerate() {
        if is_retraction_or_retracted(&sentences, index) {
            continue;
        }
        retained.extend(
            required_sentence_words(&sentence.words)
                .iter()
                .filter(|word| !allow_filler_removal || !is_filler_word(word))
                .cloned(),
        );
    }
    retained
}

fn is_explicit_negation_word(word: &str) -> bool {
    matches!(
        word,
        "not"
            | "no"
            | "cannot"
            | "nothing"
            | "nobody"
            | "none"
            | "nor"
            | "neither"
            | "never"
            | "without"
            | "non"
            | "ne"
            | "pas"
            | "jamais"
            | "aucun"
            | "aucune"
            | "sans"
            | "rien"
            | "ni"
    )
}

fn is_english_contraction(pair: &[String]) -> bool {
    pair[1] == "t"
        && matches!(
            pair[0].as_str(),
            "aren"
                | "can"
                | "couldn"
                | "didn"
                | "doesn"
                | "don"
                | "hadn"
                | "hasn"
                | "haven"
                | "isn"
                | "mustn"
                | "shouldn"
                | "wasn"
                | "weren"
                | "won"
                | "wouldn"
        )
}

fn contains_french_n_apostrophe(text: &str) -> bool {
    text.to_lowercase()
        .split(|ch: char| !ch.is_alphabetic() && !matches!(ch, '\'' | '’'))
        .any(|token| token.starts_with("n'") || token.starts_with("n’"))
}

/// Returns whether required content contains explicit negation.
fn contains_negation(text: &str, words: &[String]) -> bool {
    words.iter().any(|word| is_explicit_negation_word(word))
        || words.windows(2).any(is_english_contraction)
        || (contains_french_n_apostrophe(text) && words.iter().any(|word| word == "n"))
}

fn is_negation_component(word: &str) -> bool {
    is_explicit_negation_word(word)
        || word == "n"
        || word == "t"
        || matches!(
            word,
            "aren"
                | "can"
                | "couldn"
                | "didn"
                | "doesn"
                | "don"
                | "hadn"
                | "hasn"
                | "haven"
                | "isn"
                | "mustn"
                | "shouldn"
                | "wasn"
                | "weren"
                | "won"
                | "wouldn"
        )
}

/// Counts semantic negation markers without double-counting French `ne … pas`.
fn negation_count(text: &str, words: &[String]) -> usize {
    let english = words
        .iter()
        .filter(|word| {
            matches!(
                word.as_str(),
                "not"
                    | "no"
                    | "cannot"
                    | "nothing"
                    | "nobody"
                    | "none"
                    | "nor"
                    | "neither"
                    | "never"
                    | "without"
            )
        })
        .count()
        + words
            .windows(2)
            .filter(|pair| is_english_contraction(pair))
            .count();
    let french_strong = words
        .iter()
        .filter(|word| {
            matches!(
                word.as_str(),
                "non" | "pas" | "jamais" | "aucun" | "aucune" | "sans" | "rien" | "ni"
            )
        })
        .count();
    let french_lead = usize::from(
        french_strong == 0
            && (words.iter().any(|word| word == "ne")
                || (contains_french_n_apostrophe(text) && words.iter().any(|word| word == "n"))),
    );

    english + french_strong + french_lead
}

/// Splits provider output into clauses so a negation retained for one
/// instruction cannot mask a negation dropped from another one.
fn normalized_clauses(output: &str) -> Vec<Vec<String>> {
    let mut clauses = Vec::new();
    for sentence in normalized_sentences(output) {
        let mut current = Vec::new();
        for word in sentence.words {
            if matches!(
                word.as_str(),
                "and" | "but" | "then" | "et" | "mais" | "puis"
            ) {
                if !current.is_empty() {
                    clauses.push(std::mem::take(&mut current));
                }
            } else {
                current.push(word);
            }
        }
        if !current.is_empty() {
            clauses.push(current);
        }
    }
    clauses
}

fn negation_lost_by_instruction(input: &str, output: &str, allow_filler_removal: bool) -> bool {
    let input_sentences = normalized_sentences(input);
    let output_clauses = normalized_clauses(output);
    let output_negations: usize = output_clauses
        .iter()
        .map(|clause| negation_count(output, clause))
        .sum();
    let mut required_negations = 0;

    for (index, sentence) in input_sentences.iter().enumerate() {
        if is_retraction_or_retracted(&input_sentences, index) {
            continue;
        }
        let words: Vec<String> = required_sentence_words(&sentence.words)
            .iter()
            .filter(|word| !allow_filler_removal || !is_filler_word(word))
            .cloned()
            .collect();
        if !contains_negation(sentence.raw, &words) {
            continue;
        }
        required_negations += 1;

        let anchors: HashSet<&str> = words
            .iter()
            .map(String::as_str)
            .filter(|word| !is_negation_component(word))
            .collect();
        let best_overlap = output_clauses
            .iter()
            .map(|clause| {
                clause
                    .iter()
                    .filter(|word| anchors.contains(word.as_str()))
                    .count()
            })
            .max()
            .unwrap_or(0);
        if best_overlap == 0
            || !output_clauses.iter().any(|clause| {
                clause
                    .iter()
                    .filter(|word| anchors.contains(word.as_str()))
                    .count()
                    == best_overlap
                    && contains_negation(output, clause)
            })
        {
            return true;
        }
    }

    required_negations > output_negations
}

/// The clean templates may repair individual words, but they must not silently
/// lose a substantive sentence or produce an obvious low-overlap rewrite such
/// as a whole-language translation. Filler words are excluded only for the
/// template that explicitly permits removing them.
fn clean_polish_lost_content(input: &str, output: &str, allow_filler_removal: bool) -> bool {
    let input_words = retained_input_words(input, allow_filler_removal);
    let output_word_list = normalized_words(output);
    let output_words: HashSet<String> = output_word_list.iter().cloned().collect();
    if input_words.is_empty() || output_words.is_empty() {
        return !input_words.is_empty();
    }

    if negation_lost_by_instruction(input, output, allow_filler_removal) {
        return true;
    }

    let retained = input_words
        .iter()
        .filter(|word| output_words.contains(*word))
        .count();
    if retained.saturating_mul(100) < input_words.len().saturating_mul(30) {
        return true;
    }

    dropped_substantive_sentence(input, &output_words, allow_filler_removal)
}

/// Applies the template-specific expansion and content-preservation checks.
fn guard_polish_content(
    template_id: &str,
    template_prompt: &str,
    input: &str,
    output: &str,
) -> Result<String, &'static str> {
    let output = clamp_polish_expansion(template_id, input, output);
    if matches!(template_id, TEMPLATE_CLEAN | TEMPLATE_NO_FILLERS)
        && default_polish_templates().into_iter().any(|template| {
            template.id == template_id && template.prompt.trim() == template_prompt.trim()
        })
        && clean_polish_lost_content(input, &output, template_id == TEMPLATE_NO_FILLERS)
    {
        Err("Dictation polish dropped source content; using raw text")
    } else {
        Ok(output)
    }
}

/// Clean / no-fillers must not invent a closing. If the model added a new
/// paragraph, keep the first one when it still matches the dictation length.
fn clamp_polish_expansion(template_id: &str, input: &str, output: &str) -> String {
    if !matches!(template_id, TEMPLATE_CLEAN | TEMPLATE_NO_FILLERS) {
        return output.to_string();
    }
    if !expanded_too_much(input, output) {
        return output.to_string();
    }
    if let Some((first, rest)) = output.split_once("\n\n") {
        let first = first.trim();
        if !first.is_empty() && !rest.trim().is_empty() && !expanded_too_much(input, first) {
            return first.to_string();
        }
    }
    input.trim().to_string()
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
) -> DictationPolishResult {
    let stripped = strip_invisible_chars(raw_text);

    if let Some(result) = early_polish_dictation_result(settings, raw_text) {
        return result;
    }

    let resolution = resolve_app_branch(settings, focused_app);
    match &resolution {
        AppBranchResolution::Rule { rule, template } => {
            tracing::debug!(rule_id = %rule.id.0, template_id = %template.id, "App branch selected");
        }
        AppBranchResolution::Global { reason, .. } => {
            tracing::debug!(?reason, "App branch uses global polish template");
        }
    }
    let Some(template) = resolution.template() else {
        return DictationPolishResult {
            text: stripped.trim().to_string(),
            skipped: true,
            warning: Some("No dictation polish template configured".into()),
        };
    };

    let model = match choose_summary_model(settings, available_models) {
        Ok(model) => model,
        Err(err) => {
            // Silence here is what made this look like a broken feature: polish
            // was on, nothing happened, and nothing was written down.
            tracing::warn!(reason = err.message(), "Dictation polish skipped");
            return DictationPolishResult {
                text: stripped.trim().to_string(),
                skipped: true,
                warning: Some(err.message().to_string()),
            };
        }
    };

    let provider = match resolve_provider(&model) {
        Ok(provider) => provider,
        Err(err) => {
            tracing::warn!(model = %model, error = %err, "Dictation polish provider unusable");
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

    let prompt = build_polish_user_prompt(&template_prompt, &stripped, dictionary, focused_app);
    let no_op = |_: SummarizeProgress| {};
    let raw = match generate_with_provider(
        provider,
        &model,
        &settings.ollama_url,
        polish_system_prompt(provider),
        prompt,
        0.1,
        super::ollama::polish_budget(super::estimate_tokens(&stripped)),
        &no_op,
        false,
    )
    .await
    {
        Ok(raw) => raw,
        Err(err) => {
            tracing::warn!(model = %model, error = %err, "Dictation polish request failed");
            return DictationPolishResult {
                text: stripped.trim().to_string(),
                skipped: false,
                warning: Some(err),
            };
        }
    };

    match parse_polish_response(&raw) {
        Ok(text) => {
            let text = super::formatters::apply_post_polish_formatters(&text);
            let text = strip_prompt_leakage(&text, &stripped);
            let text = match guard_polish_content(
                template.id.as_str(),
                &template_prompt,
                &stripped,
                &text,
            ) {
                Ok(text) => text,
                Err(warning) => {
                    tracing::warn!(model = %model, warning, "Dictation polish rejected");
                    return DictationPolishResult {
                        text: stripped.trim().to_string(),
                        skipped: false,
                        warning: Some(warning.to_string()),
                    };
                }
            };
            if text.trim().is_empty() {
                DictationPolishResult {
                    text: stripped.trim().to_string(),
                    skipped: false,
                    warning: Some("Dictation polish returned empty text".into()),
                }
            } else {
                DictationPolishResult {
                    text,
                    skipped: false,
                    warning: None,
                }
            }
        }
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
        TEMPLATE_NO_FILLERS, build_polish_user_prompt, clamp_polish_expansion,
        default_polish_templates, early_polish_dictation_result, effective_template_prompt,
        guard_polish_content, is_blank_for_polish, is_own_app_name, merge_polish_templates,
        parse_polish_response, strip_invisible_chars, strip_prompt_leakage,
        superseded_default_prompts,
    };
    use crate::filter::DictionaryEntry;
    use crate::settings::{AppSettings, DictationPolishTemplate};

    #[test]
    fn app_branch_shared_chat_and_code_presets_are_available() {
        let templates = default_polish_templates();
        assert!(templates.iter().any(|template| template.id == "chat"));
        assert!(templates.iter().any(|template| template.id == "code"));
    }

    fn branch(pattern: &str, template: &str, enabled: bool) -> crate::settings::AppBranchRule {
        let mut rule = crate::settings::AppBranchRule::new(
            pattern.into(),
            crate::settings::AppBranchTarget::Template(crate::settings::PolishTemplateId(
                template.into(),
            )),
        );
        rule.enabled = enabled;
        rule
    }

    #[test]
    fn app_branch_first_active_valid_substring_wins_and_reorder_changes_winner() {
        let mut settings = AppSettings {
            dictation_app_branches: vec![
                branch("mail", "chat", false),
                branch("MAIL", "email", true),
                branch("Apple", "code", true),
            ],
            ..AppSettings::default()
        };
        assert_eq!(
            super::resolve_app_branch(&settings, Some("Apple Mail"))
                .template()
                .unwrap()
                .id,
            "email"
        );
        settings.dictation_app_branches.swap(1, 2);
        assert_eq!(
            super::resolve_app_branch(&settings, Some("Apple Mail"))
                .template()
                .unwrap()
                .id,
            "code"
        );
        settings.dictation_app_branches[1].enabled = false;
        assert_eq!(
            super::resolve_app_branch(&settings, Some("Apple Mail"))
                .template()
                .unwrap()
                .id,
            "email"
        );
    }

    #[test]
    fn app_branch_invalid_rules_cannot_mask_valid_match_and_fallback_is_global() {
        let mut settings = AppSettings {
            dictation_polish_template_id: "bullets".into(),
            dictation_app_branches: vec![
                branch("", "chat", true),
                branch("Mail", "deleted", true),
                branch("mail", "email", true),
            ],
            ..AppSettings::default()
        };
        assert_eq!(
            super::resolve_app_branch(&settings, Some("MAIL"))
                .template()
                .unwrap()
                .id,
            "email"
        );
        settings.dictation_app_branches.pop();
        for app in [None, Some(""), Some("Finder"), Some("Mail")] {
            assert_eq!(
                super::resolve_app_branch(&settings, app)
                    .template()
                    .unwrap()
                    .id,
                "bullets"
            );
        }
        assert!(matches!(
            super::resolve_app_branch(&settings, Some("Mail")),
            super::AppBranchResolution::Global {
                reason: super::AppBranchFallback::MissingTemplate,
                ..
            }
        ));
    }

    #[test]
    fn app_branch_global_target_follows_global_and_shared_edits_reach_all_branches() {
        let mut settings = AppSettings::default();
        let mut global = branch("Mail", "email", true);
        global.target = crate::settings::AppBranchTarget::Global;
        settings.dictation_app_branches = vec![
            global,
            branch("Slack", "chat", true),
            branch("Messages", "chat", true),
        ];
        settings.dictation_polish_template_id = "code".into();
        assert_eq!(
            super::resolve_app_branch(&settings, Some("Mail"))
                .template()
                .unwrap()
                .id,
            "code"
        );
        settings
            .dictation_polish_templates
            .iter_mut()
            .find(|template| template.id == "chat")
            .unwrap()
            .prompt = "New shared instruction".into();
        for app in ["Slack", "Messages"] {
            assert_eq!(
                super::resolve_app_branch(&settings, Some(app))
                    .template()
                    .unwrap()
                    .prompt,
                "New shared instruction"
            );
        }
    }

    #[test]
    fn app_branch_polish_off_has_no_effect_and_stop_snapshot_remains_authoritative() {
        let mut settings = AppSettings {
            dictation_app_branches: vec![
                branch("Mail", "email", true),
                branch("Code", "code", true),
            ],
            ..AppSettings::default()
        };
        let mut frontmost = "Mail".to_owned();
        let captured_at_stop = frontmost.clone();
        frontmost = "Code".into();
        assert_eq!(
            super::resolve_app_branch(&settings, Some(&captured_at_stop))
                .template()
                .unwrap()
                .id,
            "email"
        );
        assert_eq!(
            super::resolve_app_branch(&settings, Some(&frontmost))
                .template()
                .unwrap()
                .id,
            "code"
        );
        settings.dictation_polish_enabled = false;
        assert!(matches!(
            super::resolve_app_branch(&settings, Some(&captured_at_stop)),
            super::AppBranchResolution::Global {
                reason: super::AppBranchFallback::PolishDisabled,
                ..
            }
        ));
        assert_eq!(
            early_polish_dictation_result(&settings, "Raw dictation")
                .unwrap()
                .text,
            "Raw dictation"
        );
    }

    #[tokio::test]
    async fn app_branch_pipeline_sends_stop_app_shared_prompt_and_global_fallback() {
        use std::io::{Read, Write};
        use std::time::{Duration, Instant};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (prompt_tx, prompt_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut generations = 0;
            while generations < 3 {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "polish request timed out");
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("accept: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = Vec::new();
                let (header_end, length) = loop {
                    let mut buffer = [0; 4096];
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0, "request closed before headers");
                    request.extend_from_slice(&buffer[..count]);
                    if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap();
                        break (end + 4, length);
                    }
                };
                while request.len() < header_end + length {
                    let mut buffer = [0; 4096];
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0, "request closed before body");
                    request.extend_from_slice(&buffer[..count]);
                }
                let body = if request.starts_with(b"POST /api/show ") {
                    r#"{"model_info":{"qwen2.context_length":32768}}"#.to_string()
                } else {
                    assert!(request.starts_with(b"POST /api/generate "));
                    let request: serde_json::Value =
                        serde_json::from_slice(&request[header_end..header_end + length]).unwrap();
                    prompt_tx
                        .send(request["prompt"].as_str().unwrap().to_string())
                        .unwrap();
                    generations += 1;
                    format!(
                        "{}\n",
                        serde_json::json!({"response": "{\"text\":\"Dictated text.\"}", "done": true})
                    )
                };
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let mut settings = AppSettings {
            ollama_url: url,
            dictation_polish_template_id: "bullets".into(),
            dictation_app_branches: vec![
                branch("Mail", "chat", true),
                branch("Messages", "chat", true),
            ],
            ..AppSettings::default()
        };
        let models = vec![super::super::SummaryModelDescriptor {
            id: "qwen2.5:7b".into(),
            label: "Fixture".into(),
            provider: super::super::SummaryProviderKind::Ollama,
            can_summarize: true,
        }];
        settings
            .dictation_polish_templates
            .iter_mut()
            .find(|template| template.id == "chat")
            .unwrap()
            .prompt = "Shared v1 instruction".into();
        let captured_at_stop = "Mail".to_owned();
        let frontmost_during_finalize = "Visual Studio Code";
        assert_ne!(captured_at_stop, frontmost_during_finalize);
        let result = super::polish_dictation_text(
            &settings,
            "Dictated text.",
            &models,
            &[],
            Some(&captured_at_stop),
        )
        .await;
        assert_eq!(result.warning, None);
        assert!(prompt_rx.recv().unwrap().contains("Shared v1 instruction"));
        settings
            .dictation_polish_templates
            .iter_mut()
            .find(|template| template.id == "chat")
            .unwrap()
            .prompt = "Shared v2 instruction".into();
        let result = super::polish_dictation_text(
            &settings,
            "Dictated text.",
            &models,
            &[],
            Some("Messages"),
        )
        .await;
        assert_eq!(result.warning, None);
        assert!(prompt_rx.recv().unwrap().contains("Shared v2 instruction"));
        settings.dictation_app_branches[0].target = crate::settings::AppBranchTarget::Template(
            crate::settings::PolishTemplateId("deleted".into()),
        );
        let result =
            super::polish_dictation_text(&settings, "Dictated text.", &models, &[], Some("Mail"))
                .await;
        assert_eq!(result.warning, None);
        assert!(prompt_rx.recv().unwrap().contains("concise bullet list"));
        server.join().unwrap();
        settings.dictation_polish_enabled = false;
        let result =
            super::polish_dictation_text(&settings, "Dictated text.", &models, &[], Some("Mail"))
                .await;
        assert!(result.skipped);
        assert_eq!(result.text, "Dictated text.");
    }

    fn dict_entry(term: &str, pronunciation: Option<&str>) -> DictionaryEntry {
        DictionaryEntry {
            id: 0,
            term: term.to_string(),
            pronunciation: pronunciation.map(str::to_string),
            category: None,
            created_at: String::new(),
        }
    }

    fn guard_builtin_polish_content(
        template_id: &str,
        input: &str,
        output: &str,
    ) -> Result<String, &'static str> {
        let prompt = default_polish_templates()
            .into_iter()
            .find(|template| template.id == template_id)
            .expect("built-in polish template")
            .prompt;
        guard_polish_content(template_id, &prompt, input, output)
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
                TEMPLATE_NO_FILLERS,
                super::TEMPLATE_CHAT,
                super::TEMPLATE_CODE
            ]
        );
    }

    #[test]
    fn merge_polish_templates_upgrades_an_unedited_superseded_builtin() {
        let current = default_polish_templates();
        for superseded in SUPERSEDED_CLEAN_PROMPTS {
            let stored = vec![DictationPolishTemplate {
                id: TEMPLATE_CLEAN.to_string(),
                label: "Clean up".to_string(),
                prompt: superseded.to_string(),
            }];
            let merged = merge_polish_templates(stored);
            assert_eq!(merged[0].prompt, current[0].prompt);
            assert!(
                merged[0].prompt.contains("misheard"),
                "the upgraded prompt must carry the repair instruction"
            );
        }
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
        assert_eq!(merged.len(), 6);
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
                TEMPLATE_NO_FILLERS,
                super::TEMPLATE_CHAT,
                super::TEMPLATE_CODE
            ]
        );
        assert_eq!(merged[1].prompt, "Edited email");
        assert_eq!(merged[2].prompt, "Edited bullets");
        assert_eq!(merged[3].prompt, "Edited fillers");
    }

    #[test]
    fn build_polish_user_prompt_includes_template_and_transcript() {
        let prompt = build_polish_user_prompt("Make bullets", "hello world", &[], None);
        assert!(prompt.contains("Make bullets"));
        assert!(prompt.contains("hello world"));
        assert!(!prompt.contains("Preferred spellings"));
        assert!(!prompt.contains("Target app:"));
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
        );
        assert!(prompt.contains("Preferred spellings / vocabulary:"));
        assert!(prompt.contains("- V6 (also heard as: vésix, vee six)"));
        assert!(prompt.contains("- Kubernetes"));
        assert!(prompt.contains("le vésix arrive"));
    }

    #[test]
    fn build_polish_user_prompt_skips_empty_dictionary_section() {
        let prompt =
            build_polish_user_prompt("Clean this", "hello", &[dict_entry("  ", None)], None);
        assert!(!prompt.contains("Preferred spellings"));
    }

    #[test]
    fn build_polish_user_prompt_includes_focused_app() {
        let prompt = build_polish_user_prompt("Clean this", "hello", &[], Some("Mail"));
        assert!(prompt.contains("Target app: Mail"));
        assert!(prompt.contains(
            "Match that app's usual tone and formatting conventions. Do not mention the app name in the output."
        ));
        let target_at = prompt.find("Target app: Mail").unwrap();
        let transcript_at = prompt.find("Dictation transcript:").unwrap();
        assert!(
            target_at < transcript_at,
            "target-app context must precede the transcript so the model cannot echo it"
        );
    }

    #[test]
    fn build_polish_user_prompt_omits_souffle_as_target_app() {
        for name in ["Soufflé", "Souffle", "soufflé", " souffle "] {
            let prompt = build_polish_user_prompt("Clean this", "hello", &[], Some(name));
            assert!(
                !prompt.contains("Target app:"),
                "own app {name:?} must not be injected as polish context"
            );
        }
        assert!(is_own_app_name("Soufflé"));
        assert!(!is_own_app_name("Mail"));
    }

    #[test]
    fn strip_prompt_leakage_drops_echoed_target_app_block() {
        let transcript = "Bonjour mesdames et messieurs, chers enfants,";
        let leaked = "Bonjour mesdames et messieurs, chers enfants,\n\n---\n\nTarget app: Soufflé\nMatch that app's usual tone and formatting conventions. Do not mention the app name in the output.";
        assert_eq!(strip_prompt_leakage(leaked, transcript), transcript);
    }

    #[test]
    fn strip_prompt_leakage_keeps_a_dictated_target_app_line() {
        let transcript = "Target app: Mail\nPlease send this";
        let output = "Target app: Mail\nPlease send this.";
        assert_eq!(strip_prompt_leakage(output, transcript), output);
    }

    #[test]
    fn clamp_polish_expansion_keeps_the_first_paragraph() {
        let input = "Bonjour mesdames et messieurs, chers enfants,";
        let output = "Bonjour, mesdames et messieurs, chers enfants,\n\nChers enfants, je vous souhaite une excellente journée. Je vous remercie pour votre attention et votre participation.";
        assert_eq!(
            clamp_polish_expansion(TEMPLATE_CLEAN, input, output),
            "Bonjour, mesdames et messieurs, chers enfants,"
        );
    }

    #[test]
    fn clean_polish_rejects_a_dropped_short_opening_sentence() {
        let input = "Dictation test. This opening sentence must stay in the pasted text.";
        let output = "This opening sentence must stay in the pasted text.";

        assert!(guard_builtin_polish_content(TEMPLATE_CLEAN, input, output).is_err());
    }

    #[test]
    fn clean_polish_rejects_a_dropped_interior_sentence() {
        let input =
            "Keep this opening. Confidential launch details. Send the memo tomorrow morning.";
        let output = "Keep this opening. Send the memo tomorrow morning.";

        assert!(guard_builtin_polish_content(TEMPLATE_CLEAN, input, output).is_err());
    }

    #[test]
    fn clean_polish_rejects_a_translation() {
        let input = "Bonjour, ceci est un test de dictée entièrement en français.";
        let output = "Hello, this is a dictation written entirely in English.";

        assert!(guard_builtin_polish_content(TEMPLATE_CLEAN, input, output).is_err());
    }

    #[test]
    fn clean_polish_allows_repairs_and_an_intentional_filler_opening_removal() {
        let repaired = guard_builtin_polish_content(
            TEMPLATE_CLEAN,
            "petit maitre a jour également les document Confluence s'il te plait",
            "Peux-tu mettre à jour également les documents Confluence s'il te plaît ?",
        );
        assert!(repaired.is_ok());

        let without_filler = guard_builtin_polish_content(
            TEMPLATE_NO_FILLERS,
            "Euh. Envoie le document à Camille demain.",
            "Envoie le document à Camille demain.",
        );
        assert!(without_filler.is_ok());
    }

    #[test]
    fn clean_polish_allows_an_explicit_self_correction() {
        let result = guard_builtin_polish_content(
            TEMPLATE_CLEAN,
            "Approve the detailed budget proposal and send every attachment to Alice and Bob by Friday. Scratch that. Reject the proposal tomorrow.",
            "Reject the proposal tomorrow.",
        );

        assert!(result.is_ok());
    }

    #[test]
    fn clean_polish_allows_an_inline_self_correction_and_guards_its_replacement() {
        let result = guard_builtin_polish_content(
            TEMPLATE_CLEAN,
            "Approve the detailed budget proposal. No wait, reject it.",
            "Reject it.",
        );
        assert!(result.is_ok());

        let missing_replacement = guard_builtin_polish_content(
            TEMPLATE_CLEAN,
            "Approve the detailed budget proposal. No wait, reject it tomorrow.",
            "Tomorrow.",
        );
        assert!(missing_replacement.is_err());
    }

    #[test]
    fn clean_polish_rejects_an_omitted_instruction_with_shared_words() {
        let input = "Please send the invoice today. Please cancel the invoice tomorrow.";
        let output = "Please send the invoice today.";

        assert!(guard_builtin_polish_content(TEMPLATE_CLEAN, input, output).is_err());
    }

    #[test]
    fn clean_polish_rejects_a_removed_negation() {
        let result = guard_builtin_polish_content(
            TEMPLATE_CLEAN,
            "Do not send the invoice today.",
            "Do send the invoice today.",
        );
        assert!(result.is_err());

        let contracted = guard_builtin_polish_content(
            TEMPLATE_CLEAN,
            "Don't send the invoice.",
            "Send the invoice.",
        );
        assert!(contracted.is_err());
    }

    #[test]
    fn clean_polish_rejects_extended_and_french_apostrophe_negation_loss() {
        let cannot = guard_builtin_polish_content(
            TEMPLATE_CLEAN,
            "You cannot send the invoice today.",
            "You can send the invoice today.",
        );
        assert!(cannot.is_err());

        let french = guard_builtin_polish_content(
            TEMPLATE_CLEAN,
            "N'envoyez le rapport demain.",
            "Envoyez le rapport demain.",
        );
        assert!(french.is_err());

        let non_negations = guard_builtin_polish_content(
            TEMPLATE_CLEAN,
            "La version N accueille une personne de plus.",
            "La version accueille une personne.",
        );
        assert!(non_negations.is_ok());
    }

    #[test]
    fn clean_polish_matches_negation_to_each_retained_instruction() {
        let result = guard_builtin_polish_content(
            TEMPLATE_CLEAN,
            "Do not send the invoice today. Do not cancel the meeting tomorrow.",
            "Do send the invoice today. Do not cancel the meeting tomorrow.",
        );

        assert!(result.is_err());
    }

    #[test]
    fn edited_builtin_prompt_skips_the_builtin_preservation_guard() {
        let result = guard_polish_content(
            TEMPLATE_CLEAN,
            "Rewrite this concisely.",
            "The first detailed instruction must remain. The second detailed instruction must remain.",
            "Keep both instructions.",
        );

        assert!(result.is_ok());
    }

    #[test]
    fn no_fillers_ignores_removed_fillers_in_the_retention_threshold() {
        let result = guard_builtin_polish_content(
            TEMPLATE_NO_FILLERS,
            "um um um hello there",
            "hello there",
        );

        assert!(result.is_ok());
    }

    #[test]
    fn clamp_polish_expansion_does_not_touch_email_template() {
        let input = "hello";
        let output = "Subject: Hello\n\nHello,\n\nI wanted to follow up.\n\nBest regards";
        assert_eq!(
            clamp_polish_expansion(TEMPLATE_EMAIL, input, output),
            output
        );
    }

    #[test]
    fn build_polish_user_prompt_omits_blank_app_context() {
        let prompt = build_polish_user_prompt("Clean this", "hello", &[], Some("  "));
        assert!(!prompt.contains("Target app:"));
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
