use crate::constants::{
    OLLAMA_BRIEF_OVERVIEW_PROMPT, OLLAMA_DETAILED_MINUTES_PROMPT, OLLAMA_SUMMARIZE_PROMPT,
};
use crate::settings::{AppSettings, SummaryTemplate};

pub const TEMPLATE_SUMMARY_DEFAULT: &str = "default";
pub const TEMPLATE_SUMMARY_DETAILED: &str = "detailed_minutes";
pub const TEMPLATE_SUMMARY_BRIEF: &str = "brief_overview";

/// Built-in summary templates shipped with the app. A template customizes
/// ONLY the final-pass system prompt of the map-reduce pipeline; map and
/// intermediate merge prompts stay fixed so headings render exactly once.
pub fn default_summary_templates() -> Vec<SummaryTemplate> {
    vec![
        SummaryTemplate {
            id: TEMPLATE_SUMMARY_DEFAULT.to_string(),
            name: "Default".to_string(),
            prompt: OLLAMA_SUMMARIZE_PROMPT.to_string(),
        },
        SummaryTemplate {
            id: TEMPLATE_SUMMARY_DETAILED.to_string(),
            name: "Detailed minutes".to_string(),
            prompt: OLLAMA_DETAILED_MINUTES_PROMPT.to_string(),
        },
        SummaryTemplate {
            id: TEMPLATE_SUMMARY_BRIEF.to_string(),
            name: "Brief overview".to_string(),
            prompt: OLLAMA_BRIEF_OVERVIEW_PROMPT.to_string(),
        },
    ]
}

pub fn is_builtin_summary_template(id: &str) -> bool {
    matches!(
        id,
        TEMPLATE_SUMMARY_DEFAULT | TEMPLATE_SUMMARY_DETAILED | TEMPLATE_SUMMARY_BRIEF
    )
}

/// Merge persisted templates with built-ins: built-ins always exist (user
/// edits to their prompt/name are kept per id, and new built-ins appear
/// after upgrades), user-created templates are kept after them in their
/// stored order.
pub fn merge_summary_templates(stored: Vec<SummaryTemplate>) -> Vec<SummaryTemplate> {
    let defaults = default_summary_templates();
    if stored.is_empty() {
        return defaults;
    }

    let mut merged = Vec::with_capacity(defaults.len() + stored.len());
    for default in defaults {
        if let Some(existing) = stored.iter().find(|t| t.id == default.id) {
            merged.push(existing.clone());
        } else {
            merged.push(default);
        }
    }
    for template in stored {
        if !is_builtin_summary_template(&template.id) {
            merged.push(template);
        }
    }
    merged
}

/// Resolve the final-pass system prompt for a summary run.
///
/// `requested` is the template id the caller picked (manual Generate); `None`
/// means "use the configured default" (auto-summary, or Generate without an
/// explicit pick). Fallback chain: requested id -> settings default id ->
/// first template -> shipped default prompt. A cleared prompt on a built-in
/// falls back to that built-in's shipped prompt.
pub fn resolve_summary_template_prompt(settings: &AppSettings, requested: Option<&str>) -> String {
    let templates = &settings.summary_templates;

    let template = requested
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .and_then(|id| templates.iter().find(|t| t.id == id))
        .or_else(|| {
            templates
                .iter()
                .find(|t| t.id == settings.default_summary_template_id)
        })
        .or_else(|| templates.first());

    let Some(template) = template else {
        return OLLAMA_SUMMARIZE_PROMPT.to_string();
    };

    let trimmed = template.prompt.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }

    default_summary_templates()
        .into_iter()
        .find(|candidate| candidate.id == template.id)
        .map(|candidate| candidate.prompt)
        .unwrap_or_else(|| OLLAMA_SUMMARIZE_PROMPT.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        TEMPLATE_SUMMARY_BRIEF, TEMPLATE_SUMMARY_DEFAULT, TEMPLATE_SUMMARY_DETAILED,
        default_summary_templates, merge_summary_templates, resolve_summary_template_prompt,
    };
    use crate::constants::OLLAMA_SUMMARIZE_PROMPT;
    use crate::settings::{AppSettings, SummaryTemplate};

    fn custom(id: &str, prompt: &str) -> SummaryTemplate {
        SummaryTemplate {
            id: id.to_string(),
            name: format!("Custom {id}"),
            prompt: prompt.to_string(),
        }
    }

    #[test]
    fn default_templates_include_shipped_ids() {
        let ids: Vec<_> = default_summary_templates()
            .iter()
            .map(|t| t.id.clone())
            .collect();
        assert_eq!(
            ids,
            vec![
                TEMPLATE_SUMMARY_DEFAULT,
                TEMPLATE_SUMMARY_DETAILED,
                TEMPLATE_SUMMARY_BRIEF
            ]
        );
    }

    #[test]
    fn builtin_prompts_render_headings_once_and_skip_extracted_sections() {
        for template in default_summary_templates() {
            // Same policy as the shipped final-pass prompt: at most one copy
            // of each heading, and never the separately-extracted sections.
            for heading in ["## Summary", "## Topics", "## Meeting Minutes"] {
                assert!(
                    template.prompt.matches(heading).count() <= 1,
                    "template {} repeats {heading}",
                    template.id
                );
            }
            assert!(!template.prompt.contains("## Decisions"));
            assert!(!template.prompt.contains("## Action Items"));
        }
    }

    #[test]
    fn merge_keeps_edits_readds_builtins_and_preserves_customs() {
        let stored = vec![
            custom("my-template", "My prompt"),
            SummaryTemplate {
                id: TEMPLATE_SUMMARY_DEFAULT.to_string(),
                name: "Renamed".to_string(),
                prompt: "Edited".to_string(),
            },
        ];
        let merged = merge_summary_templates(stored);

        assert_eq!(merged.len(), 4);
        assert_eq!(merged[0].id, TEMPLATE_SUMMARY_DEFAULT);
        assert_eq!(merged[0].prompt, "Edited");
        assert_eq!(merged[0].name, "Renamed");
        assert_eq!(merged[1].id, TEMPLATE_SUMMARY_DETAILED);
        assert_eq!(merged[2].id, TEMPLATE_SUMMARY_BRIEF);
        assert_eq!(merged[3].id, "my-template");
    }

    #[test]
    fn merge_empty_returns_defaults() {
        assert_eq!(merge_summary_templates(Vec::new()), default_summary_templates());
    }

    #[test]
    fn resolve_requested_id_wins() {
        let mut settings = AppSettings::default();
        settings.summary_templates.push(custom("mine", "Do it my way"));

        assert_eq!(
            resolve_summary_template_prompt(&settings, Some("mine")),
            "Do it my way"
        );
    }

    #[test]
    fn resolve_unknown_or_missing_id_falls_back_to_settings_default() {
        let mut settings = AppSettings::default();
        settings.summary_templates.push(custom("mine", "Do it my way"));
        settings.default_summary_template_id = "mine".to_string();

        assert_eq!(
            resolve_summary_template_prompt(&settings, None),
            "Do it my way"
        );
        assert_eq!(
            resolve_summary_template_prompt(&settings, Some("no-such-template")),
            "Do it my way"
        );
    }

    #[test]
    fn resolve_unknown_default_falls_back_to_first_template() {
        let settings = AppSettings {
            default_summary_template_id: "gone".to_string(),
            ..Default::default()
        };

        assert_eq!(
            resolve_summary_template_prompt(&settings, None),
            OLLAMA_SUMMARIZE_PROMPT
        );
    }

    #[test]
    fn resolve_cleared_builtin_prompt_falls_back_to_shipped_prompt() {
        let mut settings = AppSettings::default();
        settings.summary_templates[0].prompt = "   ".to_string();

        assert_eq!(
            resolve_summary_template_prompt(&settings, Some(TEMPLATE_SUMMARY_DEFAULT)),
            OLLAMA_SUMMARIZE_PROMPT
        );
    }

    #[test]
    fn resolve_no_templates_falls_back_to_shipped_prompt() {
        let settings = AppSettings {
            summary_templates: Vec::new(),
            ..AppSettings::default()
        };
        assert_eq!(
            resolve_summary_template_prompt(&settings, None),
            OLLAMA_SUMMARIZE_PROMPT
        );
    }
}
