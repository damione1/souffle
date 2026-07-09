use serde::Deserialize;

use crate::transcript::{MeetingParticipant, StructuredActionItem, StructuredSummary};

use super::{
    SummaryProviderKind, SummarizeProgress, build_structured_extract_prompt, generate_with_provider,
    resolve_provider,
};

#[derive(Debug, Deserialize)]
struct StructuredSummaryWire {
    #[serde(default)]
    decisions: Vec<String>,
    #[serde(default)]
    action_items: Vec<ActionItemWire>,
    #[serde(default)]
    open_questions: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ActionItemWire {
    Object {
        text: String,
        #[serde(default)]
        owner: Option<String>,
    },
    Text(String),
}

impl ActionItemWire {
    fn into_action_item(self) -> Option<StructuredActionItem> {
        match self {
            ActionItemWire::Object { text, owner } => {
                let text = text.trim().to_string();
                if text.is_empty() {
                    return None;
                }
                Some(StructuredActionItem {
                    text,
                    owner: owner.and_then(|o| {
                        let trimmed = o.trim().to_string();
                        if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed)
                        }
                    }),
                })
            }
            ActionItemWire::Text(text) => {
                let text = text.trim().to_string();
                if text.is_empty() {
                    None
                } else {
                    Some(StructuredActionItem { text, owner: None })
                }
            }
        }
    }
}

fn trim_string_list(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn wire_to_structured_summary(wire: StructuredSummaryWire) -> StructuredSummary {
    StructuredSummary {
        decisions: trim_string_list(wire.decisions),
        action_items: wire
            .action_items
            .into_iter()
            .filter_map(ActionItemWire::into_action_item)
            .collect(),
        open_questions: trim_string_list(wire.open_questions),
    }
}

/// Strip optional markdown code fences and isolate the JSON object payload.
pub fn extract_json_payload(raw: &str) -> &str {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let body = rest
            .strip_prefix("json")
            .or_else(|| rest.strip_prefix("JSON"))
            .unwrap_or(rest);
        if let Some(end) = body.rfind("```") {
            return body[..end].trim();
        }
        return body.trim();
    }
    trimmed
}

pub fn parse_structured_summary_response(raw: &str) -> Result<StructuredSummary, String> {
    let payload = extract_json_payload(raw);
    let wire: StructuredSummaryWire = serde_json::from_str(payload)
        .map_err(|e| format!("Parse structured summary JSON: {e}"))?;
    Ok(wire_to_structured_summary(wire))
}

/// Second LLM pass: extract typed decisions, action items, and open questions
/// from the prose summary produced by the first pass.
pub async fn extract_structured_summary(
    prose_summary: &str,
    notes: Option<&str>,
    participants: &[MeetingParticipant],
    model: &str,
    ollama_base_url: Option<&str>,
) -> Result<StructuredSummary, String> {
    let provider = resolve_provider(model)?;
    let ollama_url = ollama_base_url.unwrap_or(crate::constants::OLLAMA_DEFAULT_URL);
    let system = match provider {
        SummaryProviderKind::Ollama => super::ollama::STRUCTURED_EXTRACT_SYSTEM_PROMPT,
        SummaryProviderKind::AppleIntelligence => super::apple::STRUCTURED_EXTRACT_SYSTEM_PROMPT,
    };
    let prompt = build_structured_extract_prompt(prose_summary, notes, participants);
    let no_op = |_: SummarizeProgress| {};
    let raw = generate_with_provider(
        provider,
        model,
        ollama_url,
        system,
        prompt,
        0.1,
        super::ollama::REDUCE_CONTEXT,
        &no_op,
    )
    .await?;
    parse_structured_summary_response(&raw)
}

#[cfg(test)]
mod tests {
    use super::{extract_json_payload, parse_structured_summary_response};

    #[test]
    fn parse_structured_summary_accepts_bare_json() {
        let parsed = parse_structured_summary_response(
            r#"{"decisions":["Ship Friday"],"action_items":[{"text":"Open PR","owner":"Alice"}],"open_questions":[]}"#,
        )
        .unwrap();
        assert_eq!(parsed.decisions, vec!["Ship Friday"]);
        assert_eq!(parsed.action_items.len(), 1);
        assert_eq!(parsed.action_items[0].owner.as_deref(), Some("Alice"));
    }

    #[test]
    fn parse_structured_summary_strips_code_fence() {
        let parsed = parse_structured_summary_response(
            "```json\n{\"decisions\":[],\"action_items\":[\"Follow up\"],\"open_questions\":[\"Budget?\"]}\n```",
        )
        .unwrap();
        assert!(parsed.decisions.is_empty());
        assert_eq!(parsed.action_items[0].text, "Follow up");
        assert_eq!(parsed.open_questions, vec!["Budget?"]);
    }

    #[test]
    fn parse_structured_summary_trims_empty_entries() {
        let parsed = parse_structured_summary_response(
            r#"{"decisions":["  ","Keep scope"],"action_items":[{"text":"  ","owner":"Bob"}],"open_questions":["  "]}"#,
        )
        .unwrap();
        assert_eq!(parsed.decisions, vec!["Keep scope"]);
        assert!(parsed.action_items.is_empty());
        assert!(parsed.open_questions.is_empty());
    }

    #[test]
    fn extract_json_payload_without_fence() {
        assert_eq!(extract_json_payload("  {\"a\":1}  "), "{\"a\":1}");
    }
}
