use serde::{Deserialize, Serialize};

const DEFAULT_URL: &str = "http://localhost:11434";

const SUMMARIZE_SYSTEM_PROMPT: &str = "\
You are a meeting summarizer. Given the following meeting transcript, produce:
1. A concise summary (2-3 paragraphs)
2. Key decisions made
3. Action items with responsible persons (if identifiable)
4. Topics discussed
Respond in the same language as the transcript.";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaStatus {
    pub available: bool,
    pub models: Vec<String>,
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
                OllamaStatus {
                    available: true,
                    models: tags.models.into_iter().map(|m| m.name).collect(),
                }
            } else {
                OllamaStatus {
                    available: true,
                    models: Vec::new(),
                }
            }
        }
        _ => OllamaStatus {
            available: false,
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
    let url = base_url.unwrap_or(DEFAULT_URL);
    let client = reqwest::Client::new();

    let body = GenerateRequest {
        model: model.to_string(),
        prompt: transcript_text.to_string(),
        system: SUMMARIZE_SYSTEM_PROMPT.to_string(),
        stream: true,
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

    Ok(full_text)
}
