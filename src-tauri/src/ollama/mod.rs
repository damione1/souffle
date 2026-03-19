use serde::{Deserialize, Serialize};

const DEFAULT_URL: &str = "http://localhost:11434";

const SUMMARIZE_SYSTEM_PROMPT: &str = "\
You summarize meeting transcripts into a factual, extractive outline.

Rules:
- Use only information explicitly stated in the transcript.
- Never infer project names, decisions, action items, deadlines, owners, next steps, or meeting outcomes.
- Do not turn greetings or a short introduction into a broader meeting narrative.
- Do not greet, thank, apologize, or address the reader.
- Do not add an introduction, conclusion, or commentary about the meeting quality.
- If the transcript is very short, output only the facts that are directly present.
- If the transcript only contains greetings, attendance, or setup, say only that.
- Keep the markdown section headings exactly as written below in English.
- Write the content of each bullet in the same language as the transcript.
- If a section has no content in the transcript, write a short \"none stated in transcript\" equivalent in the same language.
- Use short, concrete bullets. No paragraphs.
- Return exactly this structure and nothing before or after it:

## Summary
- ...

## Decisions
- ...

## Action Items
- ...

## Topics
- ...";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaStatus {
    pub available: bool,
    pub models: Vec<String>,
    pub summary_models: Vec<String>,
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

#[derive(Debug, Clone, Serialize)]
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

fn summary_models(models: &[String]) -> Vec<String> {
    let mut filtered = models
        .iter()
        .filter(|model| is_summary_capable_model(model))
        .cloned()
        .collect::<Vec<_>>();

    filtered.sort_by(|left, right| {
        summary_model_priority(left)
            .cmp(&summary_model_priority(right))
            .then_with(|| left.cmp(right))
    });

    filtered
}

/// Check if Ollama is running and list available models.
pub async fn check_available(base_url: Option<&str>) -> OllamaStatus {
    let url = base_url.unwrap_or(DEFAULT_URL);
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
                    summary_models: summary_models(&models),
                    models,
                }
            } else {
                OllamaStatus {
                    available: true,
                    models: Vec::new(),
                    summary_models: Vec::new(),
                }
            }
        }
        _ => OllamaStatus {
            available: false,
            models: Vec::new(),
            summary_models: Vec::new(),
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

    let url = base_url.unwrap_or(DEFAULT_URL);
    let client = reqwest::Client::new();

    let body = GenerateRequest {
        model: model.to_string(),
        prompt: format!("Transcript:\n---\n{transcript_text}\n---"),
        system: SUMMARIZE_SYSTEM_PROMPT.to_string(),
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
    use super::{is_summary_capable_model, sanitize_summary, summary_models};

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
        let ordered = summary_models(&[
            "custom-model:latest".to_string(),
            "mistral:7b".to_string(),
            "qwen2.5:7b".to_string(),
        ]);

        assert_eq!(
            ordered,
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
}
