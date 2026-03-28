use serde::{Deserialize, Serialize};

use crate::constants::{OLLAMA_DEFAULT_URL, OLLAMA_SUMMARIZE_PROMPT};

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct OllamaModelDescriptor {
    pub id: String,
    pub label: String,
    pub can_summarize: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct OllamaStatus {
    pub available: bool,
    pub base_url: String,
    pub models: Vec<OllamaModelDescriptor>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TagsResponse {
    models: Vec<ModelInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ModelInfo {
    name: String,
}

#[derive(Debug, Serialize)]
struct GenerateRequest {
    model: String,
    prompt: String,
    system: String,
    stream: bool,
    options: GenerateOptions,
}

#[derive(Debug, Serialize)]
struct GenerateOptions {
    temperature: f32,
}

#[derive(Debug, Deserialize)]
struct GenerateChunk {
    response: String,
    done: bool,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct SummarizeProgress {
    pub text: String,
    pub done: bool,
}

fn sanitize_summary(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let heading_prefixes = [
        "## ",
        "# ",
        "**Résumé",
        "**Summary",
        "**Synthèse",
        "**Decisions",
        "**Décisions",
    ];

    let mut offset = 0usize;
    for line in trimmed.lines() {
        let candidate = line.trim_start();
        if offset > 0
            && heading_prefixes
                .iter()
                .any(|prefix| candidate.starts_with(prefix))
        {
            return trimmed[offset..].trim_start().to_string();
        }
        offset += line.len() + 1;
    }

    trimmed.to_string()
}

fn is_summary_capable_model(model: &str) -> bool {
    let model = model.trim();
    if model.is_empty() {
        return false;
    }

    let lower = model.to_ascii_lowercase();
    let blocked_keywords = [
        "whisper",
        "stt",
        "asr",
        "speech",
        "wav2vec",
        "embed",
        "embedding",
        "minilm",
        "bge",
        "gte",
        "e5",
    ];

    !blocked_keywords
        .iter()
        .any(|keyword| lower.contains(keyword))
}

fn summary_model_priority(model: &str) -> usize {
    let lower = model.to_ascii_lowercase();
    if lower.contains("qwen") {
        0
    } else if lower.contains("llama") {
        1
    } else if lower.contains("mistral") {
        2
    } else if lower.contains("gemma") {
        3
    } else if lower.contains("phi") {
        4
    } else if lower.contains("deepseek") {
        5
    } else if lower.contains("command-r") || lower.contains("command r") {
        6
    } else {
        10
    }
}

fn model_descriptors(models: &[String]) -> Vec<OllamaModelDescriptor> {
    let mut descriptors = models
        .iter()
        .map(|model| OllamaModelDescriptor {
            id: model.clone(),
            label: model.clone(),
            can_summarize: is_summary_capable_model(model),
        })
        .collect::<Vec<_>>();

    descriptors.sort_by(|left, right| {
        summary_model_priority(&left.id)
            .cmp(&summary_model_priority(&right.id))
            .then_with(|| left.id.cmp(&right.id))
    });

    descriptors
}

/// Check if Ollama is running and list available models.
pub async fn check_available(base_url: Option<&str>) -> OllamaStatus {
    let url = base_url.unwrap_or(OLLAMA_DEFAULT_URL);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default();

    match client.get(format!("{url}/api/tags")).send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(tags) = resp.json::<TagsResponse>().await {
                let models = tags.models.into_iter().map(|m| m.name).collect::<Vec<_>>();
                OllamaStatus {
                    available: true,
                    base_url: url.to_string(),
                    models: model_descriptors(&models),
                }
            } else {
                OllamaStatus {
                    available: true,
                    base_url: url.to_string(),
                    models: Vec::new(),
                }
            }
        }
        _ => OllamaStatus {
            available: false,
            base_url: url.to_string(),
            models: Vec::new(),
        },
    }
}

/// Stream a summary of the transcript from Ollama.
/// Calls `on_chunk` for each streaming token.
pub async fn summarize_stream(
    transcript_text: &str,
    model: &str,
    base_url: Option<&str>,
    on_chunk: impl Fn(SummarizeProgress),
) -> Result<String, String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("No Ollama model selected for summarization".into());
    }
    if !is_summary_capable_model(model) {
        return Err(format!(
            "Model '{model}' is not suitable for meeting summaries. Choose a text-generation model such as qwen, llama, mistral, gemma, phi, or deepseek."
        ));
    }

    let url = base_url.unwrap_or(OLLAMA_DEFAULT_URL);
    let client = reqwest::Client::new();

    let body = GenerateRequest {
        model: model.to_string(),
        prompt: format!("Transcript:\n---\n{transcript_text}\n---"),
        system: OLLAMA_SUMMARIZE_PROMPT.to_string(),
        stream: true,
        options: GenerateOptions { temperature: 0.0 },
    };

    let resp = client
        .post(format!("{url}/api/generate"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Ollama request: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Ollama error: {}", resp.status()));
    }

    let mut full_text = String::new();
    let mut stream = resp.bytes_stream();

    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("Stream read: {e}"))?;
        let text = String::from_utf8_lossy(&bytes);

        // Ollama sends newline-delimited JSON
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            if let Ok(parsed) = serde_json::from_str::<GenerateChunk>(line) {
                full_text.push_str(&parsed.response);
                on_chunk(SummarizeProgress {
                    text: parsed.response,
                    done: parsed.done,
                });
            }
        }
    }

    Ok(sanitize_summary(&full_text))
}

#[cfg(test)]
mod tests {
    use super::{is_summary_capable_model, model_descriptors, sanitize_summary};

    #[test]
    fn rejects_speech_and_embedding_models_for_summary() {
        assert!(!is_summary_capable_model("karanchopda333/whisper:latest"));
        assert!(!is_summary_capable_model("nomic-embed-text:latest"));
    }

    #[test]
    fn accepts_chat_models_for_summary() {
        assert!(is_summary_capable_model("qwen2.5:7b-instruct"));
        assert!(is_summary_capable_model("llama3.1:8b"));
    }

    #[test]
    fn prioritizes_common_instruction_models() {
        let ordered = model_descriptors(&[
            "custom-model:latest".to_string(),
            "mistral:7b".to_string(),
            "qwen2.5:7b".to_string(),
        ]);

        assert_eq!(
            ordered
                .into_iter()
                .map(|model| model.id)
                .collect::<Vec<_>>(),
            vec![
                "qwen2.5:7b".to_string(),
                "mistral:7b".to_string(),
                "custom-model:latest".to_string()
            ]
        );
    }

    #[test]
    fn strips_intro_before_structured_summary() {
        let text = "Thanks for the transcript.\n\n## Summary\n- Bonjour\n";
        assert_eq!(sanitize_summary(text), "## Summary\n- Bonjour");
    }

    #[test]
    fn empty_model_rejected() {
        assert!(!is_summary_capable_model(""));
    }

    #[test]
    fn whitespace_model_rejected() {
        assert!(!is_summary_capable_model("   "));
    }

    #[test]
    fn sanitize_summary_empty() {
        assert_eq!(sanitize_summary(""), "");
    }

    #[test]
    fn sanitize_summary_no_heading() {
        assert_eq!(
            sanitize_summary("Just a normal summary."),
            "Just a normal summary."
        );
    }

    #[test]
    fn sanitize_summary_multi_intro() {
        // The first line is not a heading prefix, but the second is — strip everything before it
        let input = "Here is a summary of the meeting.\n## Summary\nKey points are listed below.";
        let result = sanitize_summary(input);
        assert_eq!(result, "## Summary\nKey points are listed below.");
    }

    #[test]
    fn model_descriptors_empty() {
        let result = model_descriptors(&[]);
        assert!(result.is_empty());
    }
}
