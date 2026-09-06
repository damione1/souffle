use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::lock_ext::MutexExt;

use serde::{Deserialize, Serialize};

use crate::constants::{
    OLLAMA_DEFAULT_URL, OLLAMA_MAP_PROMPT, OLLAMA_MERGE_PROMPT, OLLAMA_STRUCTURED_EXTRACT_PROMPT,
};

const CONNECT_TIMEOUT_SECS: u64 = 5;
const READ_TIMEOUT_SECS: u64 = 120;
/// Whole-request deadline for `/api/show`. The shared client only has a
/// recurring `read_timeout`, which resets on every successful read, so a slow
/// trickle of bytes could hold up the summary path indefinitely. This lookup
/// must fail fast and fall back to the stage budget.
const SHOW_TIMEOUT_SECS: u64 = 5;

/// Bounds one `/api/generate` call. `num_ctx` is the combined prompt and
/// response window; `num_predict` caps how many tokens the model may emit.
/// Ollama truncates a prompt over `num_ctx` from the LEFT (system prompt and
/// the start of the transcript go first) and still returns 200 OK, so an
/// unbounded `num_predict` just lets generation run until `llama-server`
/// shifts the context window mid-response instead. Never set `num_ctx`
/// above the model's native context: Ollama will RoPE-extend past it, which
/// degrades coherence with no warning either. See `resolve_num_ctx`.
#[derive(Debug, Clone, Copy)]
pub struct GenerationBudget {
    pub num_ctx: u32,
    pub num_predict: u32,
}

/// Map-stage outputs measured 128 to 314 tokens; 700 is headroom, not a cap
/// expected to bite.
pub const MAP_BUDGET: GenerationBudget = GenerationBudget {
    num_ctx: 8192,
    num_predict: 700,
};

/// The final pass renders the summary templates, and the detailed-minutes one
/// asks for one bullet per distinct point and to be thorough rather than
/// terse. This bound exists to stop a runaway generation before it saturates
/// the context and starts scrolling the prompt, not to cap a legitimate
/// summary: 4096 is far above any real one, and still leaves three quarters
/// of the window for the prompt.
pub const REDUCE_BUDGET: GenerationBudget = GenerationBudget {
    num_ctx: 16384,
    num_predict: 4096,
};

/// Polish rewrites its input, so its output length tracks the input's. A fixed
/// bound would cut a long dictation off mid-sentence, which is worse than the
/// unbounded generation this guard rail replaces. Twice the input estimate
/// leaves room for the punctuation and formatting polish adds.
pub fn polish_budget(input_tokens: usize) -> GenerationBudget {
    GenerationBudget {
        num_ctx: REDUCE_BUDGET.num_ctx,
        num_predict: input_tokens.saturating_mul(2).clamp(512, 8192) as u32,
    }
}

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
    num_predict: u32,
}

#[derive(Debug, Deserialize)]
struct GenerateChunk {
    response: String,
    done: bool,
    /// Present only on the final chunk (`done: true`). Ollama sets it to the
    /// number of prompt tokens it actually evaluated; when it reaches
    /// `num_ctx - 1` the prompt was truncated (see `generate_stream`).
    #[serde(default)]
    prompt_eval_count: Option<u32>,
    /// Present only on the final chunk. Used by the map-stage empty-output
    /// net: qwen2.5:7b returns literally "-" with `eval_count` 2.
    #[serde(default)]
    eval_count: Option<u32>,
}

/// One `/api/generate` completion, including the generation-token count
/// when Ollama reported it (final NDJSON line only).
#[derive(Debug, Clone)]
pub struct GenerateOutput {
    pub text: String,
    pub eval_count: Option<u32>,
}

#[derive(Debug, Deserialize, Default)]
struct ShowResponse {
    #[serde(default)]
    model_info: serde_json::Value,
}

/// Ollama prefixes this key by architecture ("qwen2.context_length",
/// "llama.context_length", ...). Match the suffix rather than hardcoding an
/// architecture, so a model built on an architecture this code has never
/// seen still resolves.
fn context_length_from_model_info(model_info: &serde_json::Value) -> Option<u32> {
    let object = model_info.as_object()?;
    object.iter().find_map(|(key, value)| {
        if !key.ends_with(".context_length") {
            return None;
        }
        let length = u32::try_from(value.as_u64()?).ok()?;
        (length > 0).then_some(length)
    })
}

/// Size the window to what this call actually needs, bounded by what the model
/// can take. `needed` is the estimated prompt plus the generation bound: below
/// it, either the prompt is truncated from the left or the response scrolls the
/// window mid-generation, both silently.
///
/// Growing past the stage budget is only safe when the native window is known,
/// because exceeding it makes Ollama RoPE-extend and degrade coherence, also
/// silently. An unknown native window therefore pins this to the stage budget,
/// which is the behavior this code had before: the lookup must never be the
/// reason a summarization gets worse or fails.
fn resolve_num_ctx(native: Option<u32>, requested: u32, needed: u32) -> u32 {
    match native {
        Some(native) => requested.max(needed).min(native),
        None => requested,
    }
}

/// `num_ctx` is prompt plus response. After the window is resolved (and
/// possibly pinned to a small native size), the requested generation bound
/// can still exceed what remains. Cap it so generation cannot scroll the
/// prompt out of the window.
fn cap_num_predict(num_ctx: u32, prompt_tokens: u32, requested: u32) -> u32 {
    requested.min(num_ctx.saturating_sub(prompt_tokens).max(1))
}

type ModelContextCache = Mutex<HashMap<(String, String), Option<u32>>>;
static MODEL_CONTEXT_CACHE: OnceLock<ModelContextCache> = OnceLock::new();

fn model_context_cache() -> &'static ModelContextCache {
    MODEL_CONTEXT_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn context_cache_key(url: &str, model: &str) -> (String, String) {
    (url.trim_end_matches('/').to_string(), model.to_string())
}

/// A completed `/api/show` (window known or advertised as absent) is cached
/// for the process lifetime. A failed lookup is not: a show that raced a
/// still-loading model must not pin every later call to the stage budget.
enum NativeContextLookup {
    Cached(Option<u32>),
    Failed,
}

/// One `/api/show` call per (server, model), cached for the process lifetime
/// so this runs once per model rather than once per chunk.
async fn native_context_length(client: &reqwest::Client, url: &str, model: &str) -> Option<u32> {
    let key = context_cache_key(url, model);
    if let Ok(cache) = model_context_cache().acquire()
        && let Some(cached) = cache.get(&key)
    {
        return *cached;
    }

    match fetch_native_context_length(client, url, model).await {
        NativeContextLookup::Cached(native) => {
            if let Ok(mut cache) = model_context_cache().acquire() {
                cache.insert(key, native);
            }
            native
        }
        NativeContextLookup::Failed => None,
    }
}

/// Failure here (network, missing field, unexpected shape) is not fatal: the
/// caller falls back to the stage's configured `num_ctx`, which is today's
/// behavior.
async fn fetch_native_context_length(
    client: &reqwest::Client,
    url: &str,
    model: &str,
) -> NativeContextLookup {
    let resp = match client
        .post(format!("{url}/api/show"))
        .timeout(Duration::from_secs(SHOW_TIMEOUT_SECS))
        .json(&serde_json::json!({ "model": model }))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            tracing::debug!(model, error = %e, "Ollama /api/show request failed");
            return NativeContextLookup::Failed;
        }
    };
    if !resp.status().is_success() {
        tracing::debug!(model, status = %resp.status(), "Ollama /api/show returned an error status");
        return NativeContextLookup::Failed;
    }
    let body: ShowResponse = match resp.json().await {
        Ok(body) => body,
        Err(e) => {
            tracing::debug!(model, error = %e, "Ollama /api/show response did not parse");
            return NativeContextLookup::Failed;
        }
    };
    NativeContextLookup::Cached(context_length_from_model_info(&body.model_info))
}

pub fn is_summary_capable_model(model: &str) -> bool {
    let model = model.trim();
    if model.is_empty() {
        return false;
    }

    let lower = model.to_ascii_lowercase();

    // Speech and embedding models cannot follow a prose instruction at all.
    // Code models can, badly: they are tuned for completion, and asked to clean
    // a dictation they tend to echo it back or comment on it. Offering one is
    // worse than offering nothing, because the feature then looks broken
    // instead of unconfigured.
    let speech_or_embedding = ["whisper", "speech", "wav2vec", "embed", "minilm"];
    // "coder" alone covers starcoder, sqlcoder, deepseek-coder and qwen-coder.
    let code_models = ["coder", "codellama", "codegemma", "codestral", "code-"];
    if speech_or_embedding
        .iter()
        .chain(code_models.iter())
        .any(|keyword| lower.contains(keyword))
    {
        return false;
    }

    let blocked_tokens = [
        "stt", "asr", "e5", "bge", "gte", "code", "audio", "omni", "vl", "vision", "tts", "xtts",
    ];
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
    eval_count: &mut Option<u32>,
    on_chunk: &impl Fn(super::SummarizeProgress),
    model: &str,
    num_ctx: u32,
) {
    let Ok(text) = std::str::from_utf8(line) else {
        return;
    };
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if let Ok(parsed) = serde_json::from_str::<GenerateChunk>(text) {
        if let Some(prompt_eval_count) = parsed.prompt_eval_count
            && prompt_eval_count >= num_ctx.saturating_sub(1)
        {
            tracing::warn!(
                model,
                num_ctx,
                prompt_eval_count,
                "Ollama prompt was truncated from the left: the system prompt and \
                 the start of the input were dropped before the model saw them"
            );
        }
        if let Some(count) = parsed.eval_count {
            *eval_count = Some(count);
        }
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
    budget: GenerationBudget,
    temperature: f32,
    on_chunk: &impl Fn(super::SummarizeProgress),
    json_format: bool,
) -> Result<GenerateOutput, String> {
    let native = native_context_length(client, url, model).await;
    let prompt_tokens = super::estimate_tokens(system)
        .saturating_add(super::estimate_tokens(&prompt))
        .try_into()
        .unwrap_or(u32::MAX);
    let needed = prompt_tokens.saturating_add(budget.num_predict);
    let num_ctx = resolve_num_ctx(native, budget.num_ctx, needed);
    let num_predict = cap_num_predict(num_ctx, prompt_tokens, budget.num_predict);
    if needed > num_ctx {
        tracing::warn!(
            model,
            needed,
            num_ctx,
            native_context = native,
            "This model's context window cannot hold the prompt and the response; \
             Ollama will drop the system prompt and the start of the input"
        );
    }

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
            num_predict,
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
    let mut eval_count = None;
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();

    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("Stream read: {e}"))?;
        buf.extend_from_slice(&bytes);

        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            handle_ndjson_line(
                &line,
                &mut full_text,
                &mut eval_count,
                on_chunk,
                model,
                num_ctx,
            );
        }
    }
    handle_ndjson_line(
        &buf,
        &mut full_text,
        &mut eval_count,
        on_chunk,
        model,
        num_ctx,
    );

    Ok(GenerateOutput {
        text: full_text,
        eval_count,
    })
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

#[cfg(test)]
mod tests {
    use super::{
        GenerateOptions, GenerateRequest, PullAccumulator, PullChunk, PullRequest,
        RECOMMENDED_MODEL, REDUCE_BUDGET, cap_num_predict, context_cache_key,
        context_length_from_model_info, is_summary_capable_model, polish_budget, resolve_num_ctx,
        sorted_summary_capable_models,
    };

    #[test]
    fn rejects_speech_and_embedding_models_for_summary() {
        assert!(!is_summary_capable_model("karanchopda333/whisper:latest"));
        assert!(!is_summary_capable_model("nomic-embed-text:latest"));
    }

    #[test]
    fn rejects_audio_omni_and_vision_models_for_summary() {
        assert!(!is_summary_capable_model("qwen2-audio"));
        assert!(!is_summary_capable_model("qwen2.5-omni"));
        assert!(!is_summary_capable_model("qwen2.5-vl:7b"));
        assert!(!is_summary_capable_model("llama3.2-vision"));
        assert!(is_summary_capable_model("qwen2.5:7b"));
        assert_eq!(
            sorted_summary_capable_models(&["qwen2-audio".into(), "qwen2.5:7b".into()]),
            vec!["qwen2.5:7b".to_string()]
        );
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
        assert!(is_summary_capable_model("audiolm:latest"));
    }

    /// A code model can follow the polish prompt just enough to look like it
    /// is working, and then echoes the dictation back unchanged. Offering one
    /// makes the feature look broken rather than unconfigured.
    #[test]
    fn rejects_code_models_for_summary() {
        assert!(!is_summary_capable_model("codellama:latest"));
        assert!(!is_summary_capable_model("qwen2.5-coder:7b"));
        assert!(!is_summary_capable_model("deepseek-coder-v2:16b"));
        assert!(!is_summary_capable_model("starcoder2:3b"));
        assert!(!is_summary_capable_model("codegemma:7b"));
        assert!(!is_summary_capable_model("codestral:22b"));
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
    fn generate_chunk_reads_eval_count_from_the_final_line() {
        let parsed: super::GenerateChunk = serde_json::from_str(
            r#"{"response":"-","done":true,"eval_count":2,"prompt_eval_count":1800}"#,
        )
        .expect("final generate line");
        assert_eq!(parsed.response, "-");
        assert_eq!(parsed.eval_count, Some(2));
        assert_eq!(parsed.prompt_eval_count, Some(1800));
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
                num_predict: 700,
            },
        };
        let json = serde_json::to_string(&body).expect("GenerateRequest should serialize");
        assert!(
            json.contains(r#""keep_alive":"15m""#),
            "expected keep_alive in {json}"
        );
    }

    #[test]
    fn polish_budget_tracks_its_input_length() {
        // A long dictation must not be cut off mid-sentence.
        assert_eq!(polish_budget(3000).num_predict, 6000);
        // A one-line dictation still gets room for punctuation and casing.
        assert_eq!(polish_budget(1).num_predict, 512);
        // Bounded, so a runaway cannot reach the context window.
        assert!(polish_budget(usize::MAX).num_predict < REDUCE_BUDGET.num_ctx);
    }

    #[test]
    fn generate_options_serializes_num_predict() {
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
                num_predict: 700,
            },
        };
        let json = serde_json::to_string(&body).expect("GenerateRequest should serialize");
        assert!(
            json.contains(r#""num_predict":700"#),
            "expected num_predict in {json}"
        );
    }

    #[test]
    fn context_length_key_matches_any_architecture_prefix() {
        let info =
            serde_json::json!({ "qwen2.context_length": 32768, "qwen2.embedding_length": 1 });
        assert_eq!(context_length_from_model_info(&info), Some(32768));

        let info = serde_json::json!({ "llama.context_length": 4096 });
        assert_eq!(context_length_from_model_info(&info), Some(4096));
    }

    #[test]
    fn context_length_key_missing_returns_none() {
        let info = serde_json::json!({ "qwen2.embedding_length": 4096 });
        assert_eq!(context_length_from_model_info(&info), None);
    }

    #[test]
    fn context_length_value_not_a_number_returns_none() {
        let info = serde_json::json!({ "qwen2.context_length": "a lot" });
        assert_eq!(context_length_from_model_info(&info), None);
    }

    #[test]
    fn context_length_zero_is_treated_as_missing() {
        let info = serde_json::json!({ "qwen2.context_length": 0 });
        assert_eq!(context_length_from_model_info(&info), None);
    }

    #[test]
    fn context_cache_key_includes_the_normalized_server() {
        assert_eq!(
            context_cache_key("http://127.0.0.1:11434/", "qwen2.5:7b"),
            context_cache_key("http://127.0.0.1:11434", "qwen2.5:7b")
        );
        assert_ne!(
            context_cache_key("http://127.0.0.1:11434", "qwen2.5:7b"),
            context_cache_key("http://192.168.0.11:11434", "qwen2.5:7b")
        );
    }

    #[test]
    fn cap_num_predict_leaves_room_for_the_prompt() {
        // A 4096-native model asked to emit 4096 tokens would otherwise
        // overflow the window and scroll the prompt mid-response.
        assert_eq!(cap_num_predict(4096, 500, 4096), 3596);
        assert_eq!(cap_num_predict(16384, 1000, 700), 700);
    }

    #[test]
    fn cap_num_predict_keeps_a_floor_when_the_prompt_already_fills_the_window() {
        assert_eq!(cap_num_predict(4096, 4096, 4096), 1);
        assert_eq!(cap_num_predict(4096, 5000, 4096), 1);
    }

    #[test]
    fn resolve_num_ctx_never_exceeds_the_native_window() {
        // RoPE extension past the native window degrades quality silently, so
        // the cap wins even when the call needs more room than that.
        assert_eq!(resolve_num_ctx(Some(4096), 8192, 2000), 4096);
        assert_eq!(resolve_num_ctx(Some(4096), 8192, 30_000), 4096);
    }

    #[test]
    fn resolve_num_ctx_stays_at_the_stage_budget_when_the_call_fits() {
        assert_eq!(resolve_num_ctx(Some(32768), 8192, 2000), 8192);
    }

    #[test]
    fn resolve_num_ctx_grows_to_fit_a_call_the_model_can_take() {
        // The whole point of reading the native window: a long reduce prompt
        // gets the room it needs instead of being truncated at a hardcoded
        // 16384.
        assert_eq!(resolve_num_ctx(Some(32768), 16384, 18_096), 18_096);
    }

    #[test]
    fn resolve_num_ctx_will_not_grow_on_an_unknown_native_window() {
        // Growing blind would risk RoPE extension, so this keeps the behavior
        // the code had before the lookup existed.
        assert_eq!(resolve_num_ctx(None, 8192, 30_000), 8192);
        assert_eq!(resolve_num_ctx(None, 8192, 2000), 8192);
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
