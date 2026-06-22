use serde::{Deserialize, Serialize};

use crate::constants::{OLLAMA_DEFAULT_URL, OLLAMA_MAP_PROMPT, OLLAMA_SUMMARIZE_PROMPT};

/// Above this estimated transcript-token count we switch from a single pass to
/// map-reduce. Single-pass on a 7B both risks Ollama's silent tail-truncation
/// and suffers "lost in the middle" — chunking materially improves whole-
/// meeting coverage (see research notes). ~6k tokens ≈ a 25-30 min meeting.
const STUFF_TOKEN_LIMIT: usize = 6000;
/// Context window for the single-pass / reduce stages. Comfortably fits ≤6k
/// tokens of input plus the generated summary, and the KV cache cost (~3GB at
/// 16k for a 7B) is fine on the target hardware.
const REDUCE_NUM_CTX: u32 = 16384;
/// Context window per map chunk — each chunk is well under this.
const MAP_NUM_CTX: u32 = 8192;
/// Target transcript words per map chunk (~2k tokens at 1.4 tokens/word) and
/// the overlap carried between consecutive chunks to preserve boundary context.
const CHUNK_WORDS: usize = 1400;
const CHUNK_OVERLAP_WORDS: usize = 120;

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
    /// Must be set explicitly on every request: Ollama's default context window
    /// is small and version-dependent, and input beyond it is silently
    /// truncated from the front — which made long meetings summarize as if only
    /// the last few minutes happened.
    num_ctx: u32,
}

#[derive(Debug, Deserialize)]
struct GenerateChunk {
    response: String,
    done: bool,
}

/// Rough token estimate without a tokenizer. Transcripts (names, fillers,
/// disfluencies) tokenize denser than prose, so use a conservative 1.4
/// tokens/word rather than the 0.75-words/token prose average.
fn estimate_tokens(text: &str) -> usize {
    (text.split_whitespace().count() as f32 * 1.4).ceil() as usize
}

/// Split a transcript into overlapping word chunks for the map stage. Overlap
/// preserves context across boundaries so a sentence cut in two is still
/// summarized coherently in at least one chunk.
fn chunk_transcript(text: &str) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() <= CHUNK_WORDS {
        return vec![text.to_string()];
    }
    let step = CHUNK_WORDS.saturating_sub(CHUNK_OVERLAP_WORDS).max(1);
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < words.len() {
        let end = (start + CHUNK_WORDS).min(words.len());
        chunks.push(words[start..end].join(" "));
        if end == words.len() {
            break;
        }
        start += step;
    }
    chunks
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

/// Build the user prompt for summarization. Notes the user took during
/// the meeting are appended as their own section so the model can weigh
/// them (decisions, action items, corrections) alongside the transcript.
pub fn build_summarize_prompt(transcript_text: &str, notes: Option<&str>) -> String {
    let mut prompt = format!("Transcript:\n---\n{transcript_text}\n---");
    if let Some(notes) = notes.map(str::trim).filter(|n| !n.is_empty()) {
        prompt.push_str(&format!(
            "\n\nUser notes (taken live during the meeting; treat them as \
             authoritative context for the summary):\n---\n{notes}\n---"
        ));
    }
    prompt
}

/// Parse one NDJSON line from Ollama's stream, appending its token to
/// `full_text` and forwarding progress. Invalid/partial UTF-8 or non-JSON
/// lines are skipped (the byte buffer only hands us complete lines).
fn handle_ndjson_line(line: &[u8], full_text: &mut String, on_chunk: &impl Fn(SummarizeProgress)) {
    let Ok(text) = std::str::from_utf8(line) else {
        return;
    };
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if let Ok(parsed) = serde_json::from_str::<GenerateChunk>(text) {
        full_text.push_str(&parsed.response);
        on_chunk(SummarizeProgress {
            text: parsed.response,
            done: parsed.done,
        });
    }
}

/// Build the reduce-stage prompt: the ordered per-chunk summaries plus an
/// explicit whole-meeting, equal-weight instruction (the chunk order is the
/// meeting order, so the model must not over-weight the final chunk).
fn build_reduce_prompt(part_summaries: &[String], notes: Option<&str>) -> String {
    let mut joined = String::new();
    for (i, part) in part_summaries.iter().enumerate() {
        joined.push_str(&format!("=== Part {} ===\n{}\n\n", i + 1, part.trim()));
    }
    let mut prompt = format!(
        "Below are ordered summaries of consecutive parts of ONE meeting \
         (Part 1 = beginning, the last part = end). Merge them into a single \
         summary that covers the whole meeting in order and gives equal weight \
         to every part.\n\nPart summaries:\n---\n{joined}---"
    );
    if let Some(notes) = notes.map(str::trim).filter(|n| !n.is_empty()) {
        prompt.push_str(&format!(
            "\n\nUser notes (taken live during the meeting; treat them as \
             authoritative context for the summary):\n---\n{notes}\n---"
        ));
    }
    prompt
}

/// One non-streaming Ollama generation; returns the full response text.
async fn generate_once(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    system: &str,
    prompt: String,
    num_ctx: u32,
    temperature: f32,
) -> Result<String, String> {
    let body = GenerateRequest {
        model: model.to_string(),
        prompt,
        system: system.to_string(),
        stream: false,
        options: GenerateOptions { temperature, num_ctx },
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
    let parsed = resp
        .json::<GenerateChunk>()
        .await
        .map_err(|e| format!("Ollama response: {e}"))?;
    Ok(parsed.response)
}

/// One streaming Ollama generation; forwards each token to `on_chunk` and
/// returns the accumulated text.
#[allow(clippy::too_many_arguments)]
async fn generate_stream(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    system: &str,
    prompt: String,
    num_ctx: u32,
    temperature: f32,
    on_chunk: &impl Fn(SummarizeProgress),
) -> Result<String, String> {
    let body = GenerateRequest {
        model: model.to_string(),
        prompt,
        system: system.to_string(),
        stream: true,
        options: GenerateOptions { temperature, num_ctx },
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
    // Buffer raw bytes and split on newlines ourselves: a network chunk can end
    // mid-line (even mid-UTF-8-codepoint for accented French), so decoding each
    // chunk independently would drop tokens — including the final `done` line,
    // which would leave the UI stuck "Generating…".
    let mut buf: Vec<u8> = Vec::new();

    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("Stream read: {e}"))?;
        buf.extend_from_slice(&bytes);

        // Drain every complete (newline-terminated) JSON line; keep the
        // trailing partial line in `buf` for the next chunk.
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            handle_ndjson_line(&line, &mut full_text, on_chunk);
        }
    }
    // Flush a final line that arrived without a trailing newline.
    handle_ndjson_line(&buf, &mut full_text, on_chunk);

    Ok(full_text)
}

/// Stream a summary of the transcript from Ollama.
///
/// Short transcripts are summarized in a single pass. Long ones use map-reduce:
/// each chunk is summarized independently ("map"), then the ordered chunk
/// summaries are merged into the final structured summary ("reduce"). This
/// avoids Ollama's silent tail-truncation and the recency bias that made long
/// meetings summarize as if only the final minutes happened. Only the final
/// pass streams to `on_chunk` (the map stage runs without live preview).
pub async fn summarize_stream(
    transcript_text: &str,
    notes: Option<&str>,
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

    // Short enough to fit comfortably in one context — summarize directly.
    if estimate_tokens(transcript_text) <= STUFF_TOKEN_LIMIT {
        let full = generate_stream(
            &client,
            url,
            model,
            OLLAMA_SUMMARIZE_PROMPT,
            build_summarize_prompt(transcript_text, notes),
            REDUCE_NUM_CTX,
            0.2,
            &on_chunk,
        )
        .await?;
        return Ok(sanitize_summary(&full));
    }

    // Map: summarize each chunk independently, concurrently.
    let chunks = chunk_transcript(transcript_text);
    let n = chunks.len();
    let maps = chunks.iter().enumerate().map(|(i, chunk)| {
        let user = format!(
            "Part {} of {}.\n\nTranscript excerpt:\n---\n{}\n---",
            i + 1,
            n,
            chunk
        );
        generate_once(&client, url, model, OLLAMA_MAP_PROMPT, user, MAP_NUM_CTX, 0.2)
    });
    let part_summaries = futures_util::future::try_join_all(maps).await?;

    // Reduce: merge the ordered part summaries into the final summary, streamed.
    let full = generate_stream(
        &client,
        url,
        model,
        OLLAMA_SUMMARIZE_PROMPT,
        build_reduce_prompt(&part_summaries, notes),
        REDUCE_NUM_CTX,
        0.3,
        &on_chunk,
    )
    .await?;
    Ok(sanitize_summary(&full))
}

#[cfg(test)]
mod tests {
    use super::{
        CHUNK_OVERLAP_WORDS, CHUNK_WORDS, build_reduce_prompt, build_summarize_prompt,
        chunk_transcript, estimate_tokens, is_summary_capable_model, model_descriptors,
        sanitize_summary,
    };

    #[test]
    fn estimate_tokens_scales_with_words() {
        assert_eq!(estimate_tokens(""), 0);
        // 10 words * 1.4 = 14
        assert_eq!(estimate_tokens(&"word ".repeat(10)), 14);
    }

    #[test]
    fn short_transcript_is_one_chunk() {
        let text = "word ".repeat(CHUNK_WORDS);
        assert_eq!(chunk_transcript(&text).len(), 1);
    }

    #[test]
    fn long_transcript_chunks_with_overlap() {
        let total = CHUNK_WORDS * 3;
        let words: Vec<String> = (0..total).map(|i| i.to_string()).collect();
        let chunks = chunk_transcript(&words.join(" "));
        assert!(chunks.len() >= 3, "expected multiple chunks, got {}", chunks.len());
        // Consecutive chunks overlap: last words of chunk 0 reappear in chunk 1.
        let step = CHUNK_WORDS - CHUNK_OVERLAP_WORDS;
        // chunk 1 starts at word `step`; it must contain word `step` and the
        // overlap word `step-1` from chunk 0 lives at the tail of chunk 0.
        assert!(chunks[1].split_whitespace().next().unwrap() == step.to_string());
        assert!(chunks[0].split_whitespace().any(|w| w == step.to_string()));
    }

    #[test]
    fn reduce_prompt_orders_parts_and_demands_equal_weight() {
        let prompt = build_reduce_prompt(&["alpha".into(), "omega".into()], None);
        assert!(prompt.contains("Part 1"));
        assert!(prompt.contains("Part 2"));
        assert!(prompt.contains("alpha"));
        assert!(prompt.contains("omega"));
        assert!(prompt.contains("equal weight"));
        assert!(prompt.find("alpha").unwrap() < prompt.find("omega").unwrap());
    }

    #[test]
    fn reduce_prompt_appends_notes() {
        let prompt = build_reduce_prompt(&["alpha".into()], Some("decision: ship"));
        assert!(prompt.contains("User notes"));
        assert!(prompt.contains("decision: ship"));
    }

    #[test]
    fn prompt_without_notes_is_transcript_only() {
        let prompt = build_summarize_prompt("hello world", None);
        assert!(prompt.contains("Transcript:\n---\nhello world\n---"));
        assert!(!prompt.contains("User notes"));
    }

    #[test]
    fn prompt_includes_user_notes_section() {
        let prompt = build_summarize_prompt("hello", Some("decision: ship friday"));
        assert!(prompt.contains("User notes"));
        assert!(prompt.contains("decision: ship friday"));
        // Transcript stays first so the notes read as added context.
        assert!(prompt.find("Transcript:").unwrap() < prompt.find("User notes").unwrap());
    }

    #[test]
    fn blank_notes_are_ignored() {
        let prompt = build_summarize_prompt("hello", Some("   "));
        assert!(!prompt.contains("User notes"));
    }

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
