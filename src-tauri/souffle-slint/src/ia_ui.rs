//! IA tab (SOU-188 milestone 5): Intelligence, DictationPolish, and
//! SummaryTemplates sections. Summary-model options keep their open-set ids
//! alongside their labels so duplicate labels never erase identity.

use crate::{AppleIntelligenceUnavailableReason as SlintAppleReason, MainWindow};
use souffle_lib::apple_intelligence::AppleIntelligenceUnavailableReason;
use souffle_lib::settings::{AppSettings, DictationPolishTemplate, SummaryTemplate};
use souffle_lib::summary::{SummaryModelDescriptor, SummaryProviderKind, SummaryProvidersStatus};

fn shared_string_vec(values: &[String]) -> slint::ModelRc<slint::SharedString> {
    let values: Vec<slint::SharedString> = values.iter().map(|v| v.as_str().into()).collect();
    std::rc::Rc::new(slint::VecModel::from(values)).into()
}

fn dictation_polish_label(template: &DictationPolishTemplate) -> String {
    match template.id.as_str() {
        "clean" => "__CLEANUP__".to_string(),
        "email" => "__PROFESSIONAL_EMAIL__".to_string(),
        "bullets" => "__BULLETS__".to_string(),
        "no_fillers" => "__NO_FILLERS__".to_string(),
        _ => template.label.clone(),
    }
}

fn summary_template_label(template: &SummaryTemplate) -> String {
    match template.id.as_str() {
        "default" => "__DEFAULT__".to_string(),
        "detailed_minutes" => "__DETAILED_MINUTES__".to_string(),
        "brief_overview" => "__BRIEF_OVERVIEW__".to_string(),
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
    let ollama_models = compatible_ollama_models(&status.models);
    window.set_settings_summary_provider(summary_provider_to_slint(settings.summary_provider));
    window.set_settings_apple_intelligence_available(status.apple_intelligence_available);
    window.set_settings_apple_intelligence_reason(
        status
            .apple_intelligence_unavailable_reason
            .map(apple_unavailable_reason_to_slint)
            .unwrap_or(SlintAppleReason::Unknown),
    );
    window.set_settings_ollama_available(status.ollama_available);
    window.set_settings_summary_model_count(ollama_models.len() as i32);

    let effective_is_apple = match settings.summary_provider {
        souffle_lib::summary::SummaryProviderChoice::AppleIntelligence => true,
        souffle_lib::summary::SummaryProviderChoice::Ollama => false,
        souffle_lib::summary::SummaryProviderChoice::Auto => status.apple_intelligence_available,
    };
    let unusable = if effective_is_apple && !status.apple_intelligence_available {
        "Apple Intelligence n'est pas disponible sur cet appareil."
    } else if !effective_is_apple && ollama_models.is_empty() {
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
    let model_ids: Vec<String> = ollama_models.iter().map(|model| model.id.clone()).collect();
    let model_labels: Vec<String> = ollama_models
        .iter()
        .map(|model| model.label.clone())
        .collect();
    window.set_settings_summary_model_picker_visible(ollama_relevant && !ollama_models.is_empty());
    window.set_settings_summary_model_ids(shared_string_vec(&model_ids));
    window.set_settings_summary_model_labels(shared_string_vec(&model_labels));
    window.set_settings_selected_summary_model_label(
        selected_summary_model_label(&status.models, &settings.ollama_model).into(),
    );
    window.set_settings_show_ollama_setup(
        ollama_relevant && status.ollama_available && ollama_models.is_empty(),
    );
    window.set_settings_recommended_ollama_model(status.recommended_ollama_model.as_str().into());
}

fn is_compatible_ollama_model(model: &SummaryModelDescriptor) -> bool {
    match model.provider {
        SummaryProviderKind::AppleIntelligence => false,
        SummaryProviderKind::Ollama => model.can_summarize,
    }
}

fn compatible_ollama_models(models: &[SummaryModelDescriptor]) -> Vec<&SummaryModelDescriptor> {
    models
        .iter()
        .filter(|model| is_compatible_ollama_model(model))
        .collect()
}

pub fn apple_unavailable_reason_to_slint(
    reason: AppleIntelligenceUnavailableReason,
) -> SlintAppleReason {
    match reason {
        AppleIntelligenceUnavailableReason::DeviceNotEligible => {
            SlintAppleReason::DeviceNotEligible
        }
        AppleIntelligenceUnavailableReason::AppleIntelligenceNotEnabled => {
            SlintAppleReason::AppleIntelligenceNotEnabled
        }
        AppleIntelligenceUnavailableReason::ModelNotReady => SlintAppleReason::ModelNotReady,
        AppleIntelligenceUnavailableReason::MacosTooOld => SlintAppleReason::MacosTooOld,
        AppleIntelligenceUnavailableReason::Stub => SlintAppleReason::Stub,
        AppleIntelligenceUnavailableReason::UnsupportedPlatform => {
            SlintAppleReason::UnsupportedPlatform
        }
        AppleIntelligenceUnavailableReason::Unknown => SlintAppleReason::Unknown,
    }
}

pub fn selected_summary_model_label(models: &[SummaryModelDescriptor], id: &str) -> String {
    models
        .iter()
        .filter(|model| is_compatible_ollama_model(model))
        .find(|model| model.id == id)
        .map(|model| model.label.clone())
        .unwrap_or_else(|| id.to_string())
}

pub fn contains_summary_model_id(models: &[SummaryModelDescriptor], id: &str) -> bool {
    models
        .iter()
        .any(|model| is_compatible_ollama_model(model) && model.id == id)
}

fn summary_provider_to_slint(
    value: souffle_lib::summary::SummaryProviderChoice,
) -> crate::SummaryProvider {
    use souffle_lib::summary::SummaryProviderChoice;
    match value {
        SummaryProviderChoice::Auto => crate::SummaryProvider::Auto,
        SummaryProviderChoice::AppleIntelligence => crate::SummaryProvider::AppleIntelligence,
        SummaryProviderChoice::Ollama => crate::SummaryProvider::Ollama,
    }
}

pub fn summary_provider_from_slint(
    value: crate::SummaryProvider,
) -> souffle_lib::summary::SummaryProviderChoice {
    use souffle_lib::summary::SummaryProviderChoice;
    match value {
        crate::SummaryProvider::Auto => SummaryProviderChoice::Auto,
        crate::SummaryProvider::AppleIntelligence => SummaryProviderChoice::AppleIntelligence,
        crate::SummaryProvider::Ollama => SummaryProviderChoice::Ollama,
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
    let ids: Vec<String> = settings
        .dictation_polish_templates
        .iter()
        .map(|template| template.id.clone())
        .collect();
    window.set_settings_dictation_polish_template_ids(shared_string_vec(&ids));
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

pub fn contains_dictation_polish_id(templates: &[DictationPolishTemplate], id: &str) -> bool {
    templates.iter().any(|template| template.id == id)
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
    let ids: Vec<String> = settings
        .summary_templates
        .iter()
        .map(|template| template.id.clone())
        .collect();
    window.set_settings_summary_template_ids(shared_string_vec(&ids));
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

pub fn contains_summary_template_id(templates: &[SummaryTemplate], id: &str) -> bool {
    templates.iter().any(|template| template.id == id)
}

#[cfg(test)]
mod tests {
    use super::{
        SlintAppleReason, apple_unavailable_reason_to_slint, compatible_ollama_models,
        contains_dictation_polish_id, contains_summary_model_id, contains_summary_template_id,
    };
    use souffle_lib::apple_intelligence::AppleIntelligenceUnavailableReason;
    use souffle_lib::settings::{DictationPolishTemplate, SummaryTemplate};
    use souffle_lib::summary::{SummaryModelDescriptor, SummaryProviderKind};

    fn model(
        id: &str,
        label: &str,
        provider: SummaryProviderKind,
        can_summarize: bool,
    ) -> SummaryModelDescriptor {
        SummaryModelDescriptor {
            id: id.into(),
            label: label.into(),
            provider,
            can_summarize,
        }
    }

    #[test]
    fn ollama_catalogue_excludes_apple_and_incompatible_models() {
        let models = vec![
            model(
                "apple-intelligence",
                "Apple Intelligence",
                SummaryProviderKind::AppleIntelligence,
                true,
            ),
            model(
                "embedding-only",
                "Embedding",
                SummaryProviderKind::Ollama,
                false,
            ),
            model("summary-model", "Résumé", SummaryProviderKind::Ollama, true),
        ];

        let projected = compatible_ollama_models(&models);
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].id, "summary-model");
        assert!(contains_summary_model_id(&models, "summary-model"));
        assert!(!contains_summary_model_id(&models, "apple-intelligence"));
        assert!(!contains_summary_model_id(&models, "embedding-only"));
    }

    #[test]
    fn duplicate_labels_keep_distinct_open_set_ids() {
        let models = vec![
            model("model-a", "Même nom", SummaryProviderKind::Ollama, true),
            model("model-b", "Même nom", SummaryProviderKind::Ollama, true),
        ];

        assert!(contains_summary_model_id(&models, "model-a"));
        assert!(contains_summary_model_id(&models, "model-b"));
    }

    #[test]
    fn duplicate_template_labels_keep_distinct_open_set_ids() {
        let polish = vec![
            DictationPolishTemplate {
                id: "polish-a".into(),
                label: "Même nom".into(),
                prompt: "A".into(),
            },
            DictationPolishTemplate {
                id: "polish-b".into(),
                label: "Même nom".into(),
                prompt: "B".into(),
            },
        ];
        let summaries = vec![
            SummaryTemplate {
                id: "summary-a".into(),
                name: "Même nom".into(),
                prompt: "A".into(),
            },
            SummaryTemplate {
                id: "summary-b".into(),
                name: "Même nom".into(),
                prompt: "B".into(),
            },
        ];

        assert!(contains_dictation_polish_id(&polish, "polish-a"));
        assert!(contains_dictation_polish_id(&polish, "polish-b"));
        assert!(contains_summary_template_id(&summaries, "summary-a"));
        assert!(contains_summary_template_id(&summaries, "summary-b"));
    }

    #[test]
    fn apple_reason_conversion_covers_every_variant() {
        use AppleIntelligenceUnavailableReason as Reason;

        let cases = [
            (
                Reason::DeviceNotEligible,
                SlintAppleReason::DeviceNotEligible,
            ),
            (
                Reason::AppleIntelligenceNotEnabled,
                SlintAppleReason::AppleIntelligenceNotEnabled,
            ),
            (Reason::ModelNotReady, SlintAppleReason::ModelNotReady),
            (Reason::MacosTooOld, SlintAppleReason::MacosTooOld),
            (Reason::Stub, SlintAppleReason::Stub),
            (
                Reason::UnsupportedPlatform,
                SlintAppleReason::UnsupportedPlatform,
            ),
            (Reason::Unknown, SlintAppleReason::Unknown),
        ];

        for (domain, slint) in cases {
            assert_eq!(apple_unavailable_reason_to_slint(domain), slint);
        }
    }
}
