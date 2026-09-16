//! IA tab (SOU-188 milestone 5): Intelligence, DictationPolish, and
//! SummaryTemplates sections. Model/template pickers forward their chosen
//! *label* back to Rust for id resolution rather than doing array lookups
//! in `.slint` - same reasoning as the audio device pickers.

use crate::MainWindow;
use souffle_lib::settings::{AppSettings, DictationPolishTemplate, SummaryTemplate};
use souffle_lib::summary::{SummaryModelDescriptor, SummaryProvidersStatus};

fn shared_string_vec(values: &[String]) -> slint::ModelRc<slint::SharedString> {
    let values: Vec<slint::SharedString> = values.iter().map(|v| v.as_str().into()).collect();
    std::rc::Rc::new(slint::VecModel::from(values)).into()
}

fn dictation_polish_label(template: &DictationPolishTemplate) -> String {
    match template.id.as_str() {
        "clean" => "Nettoyage".to_string(),
        "email" => "E-mail professionnel".to_string(),
        "bullets" => "Puces".to_string(),
        "no_fillers" => "Sans fillers".to_string(),
        _ => template.label.clone(),
    }
}

fn summary_template_label(template: &SummaryTemplate) -> String {
    match template.id.as_str() {
        "default" => "Par défaut".to_string(),
        "detailed_minutes" => "Compte rendu détaillé".to_string(),
        "brief_overview" => "Aperçu bref".to_string(),
        _ => template.name.clone(),
    }
}

const SUMMARY_TEMPLATE_BUILT_INS: [&str; 3] = ["default", "detailed_minutes", "brief_overview"];

pub fn is_builtin_summary_template(id: &str) -> bool {
    SUMMARY_TEMPLATE_BUILT_INS.contains(&id)
}

/// Pushes the provider/model picker state from a real
/// `check_summary_providers()` result. `effective`/`unusable` mirror
/// `effectiveProvider`/`unusableKey` in the Svelte controller, simplified to
/// one generic message per case instead of per-reason-code wording.
pub fn populate_intelligence(
    window: &MainWindow,
    settings: &AppSettings,
    status: &SummaryProvidersStatus,
) {
    window
        .set_settings_summary_provider(summary_provider_to_str(&settings.summary_provider).into());
    window.set_settings_apple_intelligence_available(status.apple_intelligence_available);
    window.set_settings_apple_intelligence_reason(
        status
            .apple_intelligence_unavailable_reason
            .clone()
            .unwrap_or_default()
            .into(),
    );
    window.set_settings_ollama_url(status.ollama_url.as_str().into());
    window.set_settings_ollama_available(status.ollama_available);
    window.set_settings_summary_model_count(status.models.len() as i32);

    let effective_is_apple = match settings.summary_provider {
        souffle_lib::summary::SummaryProviderChoice::AppleIntelligence => true,
        souffle_lib::summary::SummaryProviderChoice::Ollama => false,
        souffle_lib::summary::SummaryProviderChoice::Auto => status.apple_intelligence_available,
    };
    let unusable = if effective_is_apple && !status.apple_intelligence_available {
        "Apple Intelligence n'est pas disponible sur cet appareil."
    } else if !effective_is_apple && status.models.is_empty() {
        "Aucun modèle Ollama compatible n'est installé."
    } else {
        ""
    };
    window.set_settings_summary_unusable_message(unusable.into());

    let ollama_relevant = matches!(
        settings.summary_provider,
        souffle_lib::summary::SummaryProviderChoice::Ollama
    ) || (matches!(
        settings.summary_provider,
        souffle_lib::summary::SummaryProviderChoice::Auto
    ) && !status.apple_intelligence_available);
    let model_labels: Vec<String> = status.models.iter().map(|m| m.label.clone()).collect();
    window.set_settings_summary_model_picker_visible(ollama_relevant && !status.models.is_empty());
    window.set_settings_summary_model_labels(shared_string_vec(&model_labels));
    let selected_label = status
        .models
        .iter()
        .find(|m| m.id == settings.ollama_model)
        .or_else(|| status.models.first())
        .map(|m| m.label.clone())
        .unwrap_or_default();
    window.set_settings_selected_summary_model_label(selected_label.into());
    window.set_settings_show_ollama_setup(
        ollama_relevant && status.ollama_available && status.models.is_empty(),
    );
    window.set_settings_recommended_ollama_model(status.recommended_ollama_model.as_str().into());
}

pub fn resolve_summary_model_id(models: &[SummaryModelDescriptor], label: &str) -> Option<String> {
    models
        .iter()
        .find(|m| m.label == label)
        .map(|m| m.id.clone())
}

fn summary_provider_to_str(value: &souffle_lib::summary::SummaryProviderChoice) -> &'static str {
    use souffle_lib::summary::SummaryProviderChoice;
    match value {
        SummaryProviderChoice::Auto => "auto",
        SummaryProviderChoice::AppleIntelligence => "apple_intelligence",
        SummaryProviderChoice::Ollama => "ollama",
    }
}

pub fn summary_provider_from_str(value: &str) -> souffle_lib::summary::SummaryProviderChoice {
    use souffle_lib::summary::SummaryProviderChoice;
    match value {
        "apple_intelligence" => SummaryProviderChoice::AppleIntelligence,
        "ollama" => SummaryProviderChoice::Ollama,
        _ => SummaryProviderChoice::Auto,
    }
}

/// Pushes the dictation-polish template picker + the active template's
/// prompt. `active_id` picks which template is shown; falls back to the
/// first template when absent (mirrors `templates.find(...) ?? templates[0]`).
pub fn populate_dictation_polish(
    window: &MainWindow,
    settings: &AppSettings,
    provider_available: bool,
) {
    window.set_settings_dictation_polish_enabled(settings.dictation_polish_enabled);
    window.set_settings_dictation_polish_provider_available(provider_available);
    let labels: Vec<String> = settings
        .dictation_polish_templates
        .iter()
        .map(dictation_polish_label)
        .collect();
    window.set_settings_dictation_polish_template_labels(shared_string_vec(&labels));

    let active = settings
        .dictation_polish_templates
        .iter()
        .find(|t| t.id == settings.dictation_polish_template_id)
        .or_else(|| settings.dictation_polish_templates.first());
    window.set_settings_active_dictation_polish_label(
        active
            .map(dictation_polish_label)
            .unwrap_or_default()
            .into(),
    );
    window.set_settings_active_dictation_polish_prompt(
        active.map(|t| t.prompt.as_str()).unwrap_or("").into(),
    );
}

pub fn resolve_dictation_polish_id(
    templates: &[DictationPolishTemplate],
    label: &str,
) -> Option<String> {
    templates
        .iter()
        .find(|t| dictation_polish_label(t) == label)
        .map(|t| t.id.clone())
}

/// Pushes the summary-template pickers. `editing_id` is which template's
/// name/prompt are shown in the edit fields - tracked in Rust
/// (`summary_templates_editing` in main.rs), defaulting to
/// `default_summary_template_id` like the Svelte controller's `$derived`.
pub fn populate_summary_templates(window: &MainWindow, settings: &AppSettings, editing_id: &str) {
    let labels: Vec<String> = settings
        .summary_templates
        .iter()
        .map(summary_template_label)
        .collect();
    window.set_settings_summary_template_labels(shared_string_vec(&labels));

    let default_label = settings
        .summary_templates
        .iter()
        .find(|t| t.id == settings.default_summary_template_id)
        .map(summary_template_label)
        .unwrap_or_default();
    window.set_settings_default_summary_template_label(default_label.into());

    let editing = settings
        .summary_templates
        .iter()
        .find(|t| t.id == editing_id)
        .or_else(|| {
            settings
                .summary_templates
                .iter()
                .find(|t| t.id == settings.default_summary_template_id)
        })
        .or_else(|| settings.summary_templates.first());
    window.set_settings_editing_summary_template_label(
        editing
            .map(summary_template_label)
            .unwrap_or_default()
            .into(),
    );
    window.set_settings_editing_summary_template_is_builtin(
        editing
            .map(|t| is_builtin_summary_template(&t.id))
            .unwrap_or(true),
    );
    window.set_settings_editing_summary_template_name(
        editing.map(|t| t.name.as_str()).unwrap_or("").into(),
    );
    window.set_settings_editing_summary_template_prompt(
        editing.map(|t| t.prompt.as_str()).unwrap_or("").into(),
    );
}

pub fn resolve_summary_template_id(templates: &[SummaryTemplate], label: &str) -> Option<String> {
    templates
        .iter()
        .find(|t| summary_template_label(t) == label)
        .map(|t| t.id.clone())
}
