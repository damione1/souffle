use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::constants::{
    OLLAMA_DEFAULT_URL, OLLAMA_MAP_PROMPT, OLLAMA_MERGE_PROMPT, OLLAMA_STRUCTURED_EXTRACT_PROMPT,
};

const REDUCE_NUM_CTX: u32 = 16384;
const MAP_NUM_CTX: u32 = 8192;
const CONNECT_TIMEOUT_SECS: u64 = 5;
const READ_TIMEOUT_SECS: u64 = 120;

/// Default chat model to offer when Ollama is running but empty.
/// `qwen2.5:7b` is instruction-tuned (~4.7 GB) and ranks first in
/// [`sorted_summary_capable_models`].
pub const RECOMMENDED_MODEL: &str = "qwen2.5:7b";

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct OllamaPullProgress {
    pub model: String,
    pub status: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub done: bool,
    pub error: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_alive: Option<String>,
    options: GenerateOptions,
}

#[derive(Debug, Serialize)]
struct GenerateOptions {
    temperature: f32,
    num_ctx: u32,
}

#[derive(Debug, Deserialize)]
struct GenerateChunk {
    response: String,
    done: bool,
}

pub fn is_summary_capable_model(model: &str) -> bool {
    let model = model.trim();
    if model.is_empty() {
        return false;
    }

    let lower = model.to_ascii_lowercase();

    let blocked_substrings = ["whisper", "speech", "wav2vec", "embed", "minilm"];
    if blocked_substrings
        .iter()
        .any(|keyword| lower.contains(keyword))
    {
        return false;
    }

    let blocked_tokens = ["stt", "asr", "e5", "bge", "gte"];
    let tokens = lower.split(|c: char| !c.is_alphanumeric());
    !tokens
        .into_iter()
        .any(|token| blocked_tokens.contains(&token))
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

pub fn sorted_summary_capable_models(models: &[String]) -> Vec<String> {
    let mut capable: Vec<String> = models
        .iter()
        .filter(|model| is_summary_capable_model(model))
        .cloned()
        .collect();
    capable.sort_by(|left, right| {
        summary_model_priority(left)
            .cmp(&summary_model_priority(right))
            .then_with(|| left.cmp(right))
    });
    capable
}

/// Check if Ollama is running and list available models.
pub async fn check_available(base_url: Option<&str>) -> (bool, Vec<String>) {
    let url = base_url.unwrap_or(OLLAMA_DEFAULT_URL);
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(_) => return (false, Vec::new()),
    };

    match client.get(format!("{url}/api/tags")).send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(tags) = resp.json::<TagsResponse>().await {
                let models = tags.models.into_iter().map(|m| m.name).collect::<Vec<_>>();
                (true, models)
            } else {
                (true, Vec::new())
            }
        }
        _ => (false, Vec::new()),
    }
}

fn handle_ndjson_line(
    line: &[u8],
    full_text: &mut String,
    on_chunk: &impl Fn(super::SummarizeProgress),
) {
    let Ok(text) = std::str::from_utf8(line) else {
        return;
    };
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if let Ok(parsed) = serde_json::from_str::<GenerateChunk>(text) {
        full_text.push_str(&parsed.response);
        // Real generation tokens only ever flow through the caller's on_chunk
        // for the truly final pass (map/intermediate reduce calls pass a
        // no-op sink instead), so this is always the "final" stage.
        on_chunk(super::SummarizeProgress {
            text: parsed.response,
            done: parsed.done,
            stage: super::SummarizeStage::Final,
            current: None,
            total: None,
        });
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn generate_stream(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    system: &str,
    prompt: String,
    num_ctx: u32,
    temperature: f32,
    on_chunk: &impl Fn(super::SummarizeProgress),
    json_format: bool,
) -> Result<String, String> {
    let body = GenerateRequest {
        model: model.to_string(),
        prompt,
        system: system.to_string(),
        stream: true,
        format: json_format.then(|| "json".to_string()),
        keep_alive: Some("15m".into()),
        options: GenerateOptions {
            temperature,
            num_ctx,
        },
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
    let mut buf: Vec<u8> = Vec::new();

    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("Stream read: {e}"))?;
        buf.extend_from_slice(&bytes);

        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            handle_ndjson_line(&line, &mut full_text, on_chunk);
        }
    }
    handle_ndjson_line(&buf, &mut full_text, on_chunk);

    Ok(full_text)
}

pub fn validate_model(model: &str) -> Result<(), String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("No Ollama model selected for summarization".into());
    }
    if !is_summary_capable_model(model) {
        return Err(format!(
            "Model '{model}' is not suitable for meeting summaries. Choose a text-generation model such as qwen, llama, mistral, gemma, phi, or deepseek."
        ));
    }
    Ok(())
}

pub fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .read_timeout(std::time::Duration::from_secs(READ_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("Ollama client: {e}"))
}

/// Pulls can idle between layers for minutes; do not apply the generate read timeout.
fn pull_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("Ollama client: {e}"))
}

#[derive(Debug, Serialize)]
struct PullRequest {
    model: String,
    stream: bool,
}

#[derive(Debug, Deserialize)]
struct PullChunk {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    total: Option<u64>,
    #[serde(default)]
    completed: Option<u64>,
    #[serde(default)]
    error: Option<String>,
}

struct PullAccumulator {
    model: String,
    status: String,
    layers: HashMap<String, (u64, u64)>,
}

impl PullAccumulator {
    fn new(model: &str) -> Self {
        Self {
            model: model.to_string(),
            status: String::new(),
            layers: HashMap::new(),
        }
    }

    fn apply(&mut self, chunk: &PullChunk) -> OllamaPullProgress {
        if let Some(status) = &chunk.status {
            self.status = status.clone();
        }
        if let Some(digest) = &chunk.digest {
            let completed = chunk.completed.unwrap_or(0);
            let total = chunk.total.unwrap_or(0).max(completed);
            self.layers.insert(digest.clone(), (completed, total));
        }
        let (downloaded, total) = self.layers.values().fold(
            (0u64, 0u64),
            |(downloaded, total), (completed, layer_total)| {
                (downloaded + completed, total + layer_total)
            },
        );
        let error = chunk
            .error
            .clone()
            .filter(|message| !message.trim().is_empty());
        let done = error.is_some() || self.status.eq_ignore_ascii_case("success");
        OllamaPullProgress {
            model: self.model.clone(),
            status: self.status.clone(),
            downloaded_bytes: downloaded,
            total_bytes: (total > 0).then_some(total),
            done,
            error,
        }
    }
}

fn handle_pull_line(
    line: &[u8],
    acc: &mut PullAccumulator,
    on_progress: &impl Fn(OllamaPullProgress),
) -> Result<(), String> {
    let Ok(text) = std::str::from_utf8(line) else {
        return Ok(());
    };
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }
    let Ok(parsed) = serde_json::from_str::<PullChunk>(text) else {
        return Ok(());
    };
    let progress = acc.apply(&parsed);
    let error = progress.error.clone();
    on_progress(progress);
    if let Some(error) = error {
        return Err(error);
    }
    Ok(())
}

/// Stream `POST /api/pull` until the model is on disk (or Ollama errors).
pub async fn pull_model(
    url: &str,
    model: &str,
    on_progress: impl Fn(OllamaPullProgress),
) -> Result<(), String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("No Ollama model specified to download".into());
    }
    if !is_summary_capable_model(model) {
        return Err(format!(
            "Model '{model}' is not suitable for summaries or dictation polish"
        ));
    }

    let client = pull_http_client()?;
    let resp = client
        .post(format!("{url}/api/pull"))
        .json(&PullRequest {
            model: model.to_string(),
            stream: true,
        })
        .send()
        .await
        .map_err(|e| format!("Ollama pull request: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let body = body.trim();
        return Err(if body.is_empty() {
            format!("Ollama pull failed ({status})")
        } else {
            format!("Ollama pull failed ({status}): {body}")
        });
    }

    let mut acc = PullAccumulator::new(model);
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut succeeded = false;

    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("Ollama pull stream: {e}"))?;
        buf.extend_from_slice(&bytes);

        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            handle_pull_line(&line, &mut acc, &on_progress)?;
            if acc.status.eq_ignore_ascii_case("success") {
                succeeded = true;
            }
        }
    }
    handle_pull_line(&buf, &mut acc, &on_progress)?;
    if acc.status.eq_ignore_ascii_case("success") {
        succeeded = true;
    }

    if succeeded {
        Ok(())
    } else {
        Err("Ollama pull ended without success".into())
    }
}

pub const MAP_SYSTEM_PROMPT: &str = OLLAMA_MAP_PROMPT;
pub const MERGE_SYSTEM_PROMPT: &str = OLLAMA_MERGE_PROMPT;
pub const STRUCTURED_EXTRACT_SYSTEM_PROMPT: &str = OLLAMA_STRUCTURED_EXTRACT_PROMPT;
pub const DICTATION_POLISH_SYSTEM_PROMPT: &str = crate::constants::OLLAMA_DICTATION_POLISH_PROMPT;
pub const REDUCE_CONTEXT: u32 = REDUCE_NUM_CTX;
pub const MAP_CONTEXT: u32 = MAP_NUM_CTX;

#[cfg(test)]
mod tests {
    use super::{
        GenerateOptions, GenerateRequest, PullAccumulator, PullChunk, PullRequest,
        RECOMMENDED_MODEL, is_summary_capable_model, sorted_summary_capable_models,
    };

    #[test]
    fn rejects_speech_and_embedding_models_for_summary() {
        assert!(!is_summary_capable_model("karanchopda333/whisper:latest"));
        assert!(!is_summary_capable_model("nomic-embed-text:latest"));
    }

    #[test]
    fn rejects_short_keyword_models_as_whole_tokens() {
        assert!(!is_summary_capable_model("intfloat/e5-large"));
        assert!(!is_summary_capable_model("kyutai-stt:1b"));
    }

    #[test]
    fn accepts_models_where_short_keyword_is_only_a_substring() {
        assert!(is_summary_capable_model("faste5ish:latest"));
        assert!(is_summary_capable_model("vgte-model:latest"));
    }

    #[test]
    fn accepts_chat_models_for_summary() {
        assert!(is_summary_capable_model("qwen2.5:7b-instruct"));
        assert!(is_summary_capable_model("llama3.1:8b"));
    }

    #[test]
    fn prioritizes_common_instruction_models() {
        let ordered = sorted_summary_capable_models(&[
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
    fn empty_model_rejected() {
        assert!(!is_summary_capable_model(""));
    }

    #[test]
    fn whitespace_model_rejected() {
        assert!(!is_summary_capable_model("   "));
    }

    #[test]
    fn generate_request_serializes_keep_alive() {
        let body = GenerateRequest {
            model: "qwen2.5:7b".into(),
            prompt: "hi".into(),
            system: "sys".into(),
            stream: true,
            format: None,
            keep_alive: Some("15m".into()),
            options: GenerateOptions {
                temperature: 0.1,
                num_ctx: 1024,
            },
        };
        let json = serde_json::to_string(&body).expect("GenerateRequest should serialize");
        assert!(
            json.contains(r#""keep_alive":"15m""#),
            "expected keep_alive in {json}"
        );
    }

    #[test]
    fn recommended_model_is_summary_capable() {
        assert!(is_summary_capable_model(RECOMMENDED_MODEL));
        assert_eq!(
            sorted_summary_capable_models(&[
                "mistral:7b".to_string(),
                RECOMMENDED_MODEL.to_string()
            ]),
            vec![RECOMMENDED_MODEL.to_string(), "mistral:7b".to_string()]
        );
    }

    #[test]
    fn pull_request_streams() {
        let json = serde_json::to_string(&PullRequest {
            model: RECOMMENDED_MODEL.into(),
            stream: true,
        })
        .expect("PullRequest should serialize");
        assert!(
            json.contains(r#""model":"qwen2.5:7b""#),
            "expected model in {json}"
        );
        assert!(
            json.contains(r#""stream":true"#),
            "expected stream in {json}"
        );
    }

    #[test]
    fn pull_accumulator_sums_layers_and_flags_success() {
        let mut acc = PullAccumulator::new(RECOMMENDED_MODEL);
        let first = acc.apply(&PullChunk {
            status: Some("downloading".into()),
            digest: Some("sha256:aaa".into()),
            total: Some(100),
            completed: Some(40),
            error: None,
        });
        assert_eq!(first.downloaded_bytes, 40);
        assert_eq!(first.total_bytes, Some(100));
        assert!(!first.done);

        let second = acc.apply(&PullChunk {
            status: Some("downloading".into()),
            digest: Some("sha256:bbb".into()),
            total: Some(50),
            completed: Some(50),
            error: None,
        });
        assert_eq!(second.downloaded_bytes, 90);
        assert_eq!(second.total_bytes, Some(150));

        let done = acc.apply(&PullChunk {
            status: Some("success".into()),
            digest: None,
            total: None,
            completed: None,
            error: None,
        });
        assert!(done.done);
        assert!(done.error.is_none());
    }

    #[test]
    fn pull_accumulator_surfaces_error() {
        let mut acc = PullAccumulator::new(RECOMMENDED_MODEL);
        let progress = acc.apply(&PullChunk {
            status: None,
            digest: None,
            total: None,
            completed: None,
            error: Some("file does not exist".into()),
        });
        assert!(progress.done);
        assert_eq!(progress.error.as_deref(), Some("file does not exist"));
    }
}
