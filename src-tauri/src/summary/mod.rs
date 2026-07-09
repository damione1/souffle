mod apple;
mod chunking;
mod extract;
mod ollama;
mod prompts;
mod reduce;

use serde::{Deserialize, Serialize};

use crate::apple_intelligence;
use crate::constants::OLLAMA_DEFAULT_URL;
use crate::transcript::MeetingParticipant;

pub use chunking::{ChunkConfig, chunk_transcript, estimate_tokens};
pub use extract::{extract_structured_summary, parse_structured_summary_response};
pub use prompts::{
    build_reduce_prompt, build_structured_extract_prompt, build_summarize_prompt,
    format_participants,
};

pub const APPLE_INTELLIGENCE_MODEL_ID: &str = "apple-intelligence";
const APPLE_INTELLIGENCE_MODEL_LABEL: &str = "Apple Intelligence";

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SummarizeProgress {
    pub text: String,
    pub done: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SummaryProviderKind {
    Ollama,
    AppleIntelligence,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SummaryModelDescriptor {
    pub id: String,
    pub label: String,
    pub provider: SummaryProviderKind,
    pub can_summarize: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SummaryProvidersStatus {
    pub ollama_url: String,
    pub ollama_available: bool,
    pub apple_intelligence_available: bool,
    /// True when this build linked the Apple Intelligence stub (no FoundationModels).
    pub apple_intelligence_is_stub: bool,
    pub models: Vec<SummaryModelDescriptor>,
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

fn apple_model_descriptor() -> SummaryModelDescriptor {
    SummaryModelDescriptor {
        id: APPLE_INTELLIGENCE_MODEL_ID.to_string(),
        label: APPLE_INTELLIGENCE_MODEL_LABEL.to_string(),
        provider: SummaryProviderKind::AppleIntelligence,
        can_summarize: true,
    }
}

pub fn apple_intelligence_available() -> bool {
    if apple_intelligence::is_stub_linked() {
        return false;
    }
    apple::validate_availability().is_ok()
}

/// List every summary provider/model currently available on this machine.
pub async fn check_providers(ollama_url: &str) -> SummaryProvidersStatus {
    let url = if ollama_url.trim().is_empty() {
        OLLAMA_DEFAULT_URL
    } else {
        ollama_url.trim()
    };

    let (ollama_available, ollama_models) = ollama::check_available(Some(url)).await;
    let apple_intelligence_is_stub = apple_intelligence::is_stub_linked();
    let apple_intelligence_available = apple_intelligence_available();

    let mut models = Vec::new();
    if apple_intelligence_available {
        models.push(apple_model_descriptor());
    }
    for model in ollama::sorted_summary_capable_models(&ollama_models) {
        models.push(SummaryModelDescriptor {
            id: model.clone(),
            label: model,
            provider: SummaryProviderKind::Ollama,
            can_summarize: true,
        });
    }

    SummaryProvidersStatus {
        ollama_url: url.to_string(),
        ollama_available,
        apple_intelligence_available,
        apple_intelligence_is_stub,
        models,
    }
}

pub(crate) fn resolve_provider(model: &str) -> Result<SummaryProviderKind, String> {
    if model.trim() == APPLE_INTELLIGENCE_MODEL_ID {
        if apple_intelligence::is_stub_linked() {
            return Err(
                "Apple Intelligence is not included in this build (FoundationModels stub). \
                 Install a build compiled with Xcode 26+ or use Ollama."
                    .into(),
            );
        }
        apple::validate_availability()?;
        return Ok(SummaryProviderKind::AppleIntelligence);
    }
    ollama::validate_model(model)?;
    Ok(SummaryProviderKind::Ollama)
}

fn chunk_config(provider: SummaryProviderKind) -> ChunkConfig {
    match provider {
        SummaryProviderKind::Ollama => ChunkConfig::OLLAMA,
        SummaryProviderKind::AppleIntelligence => ChunkConfig::APPLE_INTELLIGENCE,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn generate_with_provider(
    provider: SummaryProviderKind,
    ollama_model: &str,
    ollama_url: &str,
    system: &str,
    prompt: String,
    temperature: f32,
    num_ctx: u32,
    on_chunk: &impl Fn(SummarizeProgress),
) -> Result<String, String> {
    match provider {
        SummaryProviderKind::Ollama => {
            let client = ollama::http_client()?;
            ollama::generate_stream(
                &client,
                ollama_url,
                ollama_model,
                system,
                prompt,
                num_ctx,
                temperature,
                on_chunk,
            )
            .await
        }
        SummaryProviderKind::AppleIntelligence => {
            let system = system.to_string();
            let full = tokio::task::spawn_blocking(move || {
                apple_intelligence::process_text_with_system_prompt(&system, &prompt, 0)
            })
            .await
            .map_err(|e| format!("Apple Intelligence task: {e}"))??;
            if !full.is_empty() {
                on_chunk(SummarizeProgress {
                    text: full.clone(),
                    done: false,
                });
            }
            on_chunk(SummarizeProgress {
                text: String::new(),
                done: true,
            });
            Ok(full)
        }
    }
}

/// Stream a summary of the transcript using the selected provider.
pub async fn summarize_stream(
    transcript_text: &str,
    notes: Option<&str>,
    participants: &[MeetingParticipant],
    model: &str,
    ollama_base_url: Option<&str>,
    on_chunk: impl Fn(SummarizeProgress),
) -> Result<String, String> {
    let provider = resolve_provider(model)?;
    let config = chunk_config(provider);
    let ollama_url = ollama_base_url.unwrap_or(OLLAMA_DEFAULT_URL);

    if estimate_tokens(transcript_text) <= config.stuff_token_limit {
        let system = match provider {
            SummaryProviderKind::Ollama => ollama::SUMMARIZE_SYSTEM_PROMPT,
            SummaryProviderKind::AppleIntelligence => apple::SUMMARIZE_SYSTEM_PROMPT,
        };
        let full = generate_with_provider(
            provider,
            model,
            ollama_url,
            system,
            build_summarize_prompt(transcript_text, notes, participants),
            0.2,
            ollama::REDUCE_CONTEXT,
            &on_chunk,
        )
        .await?;
        return Ok(sanitize_summary(&full));
    }

    let no_op = |_: SummarizeProgress| {};
    let chunks = chunk_transcript(transcript_text, config);
    let n = chunks.len();
    let mut part_summaries = Vec::with_capacity(n);

    if provider == SummaryProviderKind::Ollama {
        use futures_util::{StreamExt, TryStreamExt};
        let client = ollama::http_client()?;
        part_summaries = futures_util::stream::iter(chunks.into_iter().enumerate())
            .map(|(i, chunk)| {
                let user = format!(
                    "Part {} of {}.\n\nTranscript excerpt:\n---\n{}\n---",
                    i + 1,
                    n,
                    chunk
                );
                ollama::generate_stream(
                    &client,
                    ollama_url,
                    model,
                    ollama::MAP_SYSTEM_PROMPT,
                    user,
                    ollama::MAP_CONTEXT,
                    0.2,
                    &no_op,
                )
            })
            .buffered(config.map_concurrency)
            .try_collect()
            .await?;
    } else {
        for (i, chunk) in chunks.into_iter().enumerate() {
            let user = format!(
                "Part {} of {}.\n\nTranscript excerpt:\n---\n{}\n---",
                i + 1,
                n,
                chunk
            );
            let part = generate_with_provider(
                provider,
                model,
                ollama_url,
                apple::MAP_SYSTEM_PROMPT,
                user,
                0.2,
                ollama::MAP_CONTEXT,
                &no_op,
            )
            .await?;
            part_summaries.push(part);
        }
    }

    let full = reduce_part_summaries(
        provider,
        model,
        ollama_url,
        &part_summaries,
        notes,
        participants,
        config,
        &on_chunk,
    )
    .await?;
    Ok(sanitize_summary(&full))
}

#[allow(clippy::too_many_arguments)]
async fn reduce_part_summaries(
    provider: SummaryProviderKind,
    model: &str,
    ollama_url: &str,
    part_summaries: &[String],
    notes: Option<&str>,
    participants: &[MeetingParticipant],
    config: ChunkConfig,
    on_chunk: &impl Fn(SummarizeProgress),
) -> Result<String, String> {
    let (system_prompt, num_ctx) = match provider {
        SummaryProviderKind::Ollama => (ollama::SUMMARIZE_SYSTEM_PROMPT, ollama::REDUCE_CONTEXT),
        SummaryProviderKind::AppleIntelligence => {
            (apple::SUMMARIZE_SYSTEM_PROMPT, ollama::REDUCE_CONTEXT)
        }
    };

    if provider == SummaryProviderKind::Ollama
        || reduce::reduce_prompt_fits(part_summaries, notes, participants, config.reduce_token_limit)
    {
        return generate_with_provider(
            provider,
            model,
            ollama_url,
            system_prompt,
            build_reduce_prompt(part_summaries, notes, participants),
            0.3,
            num_ctx,
            on_chunk,
        )
        .await;
    }

    let mut current: Vec<String> = part_summaries.to_vec();
    loop {
        if reduce::reduce_prompt_fits(&current, notes, participants, config.reduce_token_limit) {
            return generate_with_provider(
                provider,
                model,
                ollama_url,
                system_prompt,
                build_reduce_prompt(&current, notes, participants),
                0.3,
                num_ctx,
                on_chunk,
            )
            .await;
        }

        let batches = reduce::partition_for_reduce(&current, config.reduce_token_limit)?;
        if batches.len() == 1 && batches[0].len() == current.len() {
            let tokens = estimate_tokens(&build_reduce_prompt(&current, None, &[]));
            return Err(format!(
                "Meeting summary is too large for Apple Intelligence after batching \
                 ({tokens} estimated tokens, limit {}). Use Ollama for very long meetings.",
                config.reduce_token_limit
            ));
        }

        let no_op = |_: SummarizeProgress| {};
        let mut next = Vec::with_capacity(batches.len());
        for batch in batches {
            let merged = generate_with_provider(
                provider,
                model,
                ollama_url,
                system_prompt,
                build_reduce_prompt(&batch, None, &[]),
                0.2,
                num_ctx,
                &no_op,
            )
            .await?;
            next.push(merged);
        }
        current = next;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        APPLE_INTELLIGENCE_MODEL_ID, SummaryProviderKind, check_providers, resolve_provider,
        sanitize_summary,
    };

    #[test]
    fn sanitize_summary_strips_intro_before_structured_summary() {
        let text = "Thanks for the transcript.\n\n## Summary\n- Bonjour\n";
        assert_eq!(sanitize_summary(text), "## Summary\n- Bonjour");
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
        let input = "Here is a summary of the meeting.\n## Summary\nKey points are listed below.";
        let result = sanitize_summary(input);
        assert_eq!(result, "## Summary\nKey points are listed below.");
    }

    #[test]
    fn resolve_provider_rejects_unknown_ollama_model() {
        assert!(resolve_provider("").is_err());
        assert!(resolve_provider("nomic-embed-text").is_err());
    }

    #[test]
    fn resolve_provider_accepts_apple_model_id() {
        if super::apple_intelligence_available() {
            assert_eq!(
                resolve_provider(APPLE_INTELLIGENCE_MODEL_ID).unwrap(),
                SummaryProviderKind::AppleIntelligence
            );
        } else {
            assert!(resolve_provider(APPLE_INTELLIGENCE_MODEL_ID).is_err());
        }
    }

    #[tokio::test]
    async fn check_providers_lists_apple_before_ollama_when_available() {
        let status = check_providers("http://127.0.0.1:1").await;
        assert!(!status.ollama_available);
        assert_eq!(
            status.apple_intelligence_is_stub,
            crate::apple_intelligence::is_stub_linked()
        );
        if status.apple_intelligence_available {
            assert_eq!(status.models[0].id, APPLE_INTELLIGENCE_MODEL_ID);
            assert_eq!(status.models[0].provider, SummaryProviderKind::AppleIntelligence);
            assert!(!status.apple_intelligence_is_stub);
        }
    }
}
