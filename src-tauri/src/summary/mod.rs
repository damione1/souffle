mod apple;
mod chunking;
mod extract;
mod ollama;
mod polish;
mod prompts;
mod reduce;
mod templates;

use serde::{Deserialize, Serialize};

use crate::apple_intelligence;
use crate::constants::OLLAMA_DEFAULT_URL;
use crate::transcript::{MeetingParticipant, StructuredSummary};

pub use chunking::{ChunkConfig, chunk_transcript, estimate_tokens};
pub use extract::{extract_structured_summary, parse_structured_summary_response};
pub use polish::{
    DictationPolishResult, TEMPLATE_BULLETS, TEMPLATE_EMAIL, TEMPLATE_NO_FILLERS,
    default_polish_templates, early_polish_dictation_result, merge_polish_templates,
    polish_dictation_text,
};
pub use prompts::{
    build_reduce_prompt, build_structured_extract_prompt, build_summarize_prompt,
    format_participants,
};
pub use templates::{
    TEMPLATE_SUMMARY_BRIEF, TEMPLATE_SUMMARY_DEFAULT, TEMPLATE_SUMMARY_DETAILED,
    default_summary_templates, is_builtin_summary_template, merge_summary_templates,
    resolve_summary_template_prompt,
};

pub const APPLE_INTELLIGENCE_MODEL_ID: &str = "apple-intelligence";
const APPLE_INTELLIGENCE_MODEL_LABEL: &str = "Apple Intelligence";

/// Prose summary is always persisted; structured extraction is best-effort.
/// On extract/parse failure, callers should save prose with no structured data.
pub fn structured_extract_for_persist(
    result: Result<StructuredSummary, String>,
) -> (Option<StructuredSummary>, Option<String>) {
    match result {
        Ok(structured) => (Some(structured), None),
        Err(err) => (None, Some(err)),
    }
}

/// Which phase of summarization a progress event belongs to, so the frontend
/// can show a live stage label ("Summarizing part 3 of 12", "Combining...",
/// "Extracting outcomes...") instead of a silent multi-minute spinner.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SummarizeStage {
    /// Per-chunk map pass over the raw transcript.
    Map,
    /// Reduce pass merging part summaries (one or more rounds).
    Combine,
    /// The single terminal pass producing the user-facing prose summary;
    /// `text` carries real generation tokens for this stage only.
    Final,
    /// Structured outcome extraction (decisions/action items/open questions)
    /// run after the prose summary is complete.
    Extract,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SummarizeProgress {
    pub text: String,
    pub done: bool,
    pub stage: SummarizeStage,
    /// 1-based index of the current unit within `stage`, when applicable
    /// (e.g. chunk number during Map, batch number during Combine).
    pub current: Option<u32>,
    pub total: Option<u32>,
}

impl SummarizeProgress {
    fn stage_marker(stage: SummarizeStage, current: Option<u32>, total: Option<u32>) -> Self {
        Self {
            text: String::new(),
            done: false,
            stage,
            current,
            total,
        }
    }
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
    /// Machine-readable reason Apple Intelligence is unavailable, `None` when available.
    pub apple_intelligence_unavailable_reason: Option<String>,
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
    let apple_intelligence_unavailable_reason = apple_intelligence::unavailable_reason();
    if !apple_intelligence_available
        && let Some(reason) = &apple_intelligence_unavailable_reason
    {
        tracing::info!(reason = %reason, "Apple Intelligence unavailable");
    }

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
        apple_intelligence_unavailable_reason,
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

pub fn pick_summary_model(
    settings: &crate::settings::AppSettings,
    models: &[SummaryModelDescriptor],
) -> Option<String> {
    if models.is_empty() {
        return None;
    }
    let preferred = settings.ollama_model.trim();
    if !preferred.is_empty() && models.iter().any(|model| model.id == preferred) {
        return Some(preferred.to_string());
    }
    models.first().map(|model| model.id.clone())
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
    json_format: bool,
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
                json_format,
            )
            .await
        }
        SummaryProviderKind::AppleIntelligence => {
            let system = system.to_string();
            // Guarded call: dedicated thread per attempt, hard wall-clock
            // timeout, one retry (see apple::generate_guarded). Without it a
            // wedged FoundationModels request blocks this future forever and
            // the summarize command never resolves.
            let full = tokio::task::spawn_blocking(move || {
                apple::generate_guarded(
                    || {
                        let system = system.clone();
                        let prompt = prompt.clone();
                        move || {
                            apple_intelligence::process_text_with_system_prompt(&system, &prompt, 0)
                        }
                    },
                    apple::REQUEST_TIMEOUT,
                    apple::RETRY_BACKOFF,
                )
            })
            .await
            .map_err(|e| format!("Apple Intelligence task: {e}"))??;
            // FoundationModels has no token-streaming FFI in this build (it needs a
            // callback-based cdecl bridge, and the real Swift side only compiles on
            // CI): deliver the whole response as one chunk. Real generation only
            // ever reaches this branch's on_chunk for the final pass, same as Ollama.
            if !full.is_empty() {
                on_chunk(SummarizeProgress {
                    text: full.clone(),
                    done: false,
                    stage: SummarizeStage::Final,
                    current: None,
                    total: None,
                });
            }
            on_chunk(SummarizeProgress {
                text: String::new(),
                done: true,
                stage: SummarizeStage::Final,
                current: None,
                total: None,
            });
            Ok(full)
        }
    }
}

/// Stream a summary of the transcript using the selected provider.
///
/// `final_system_prompt` is the user-selected summary template (resolved via
/// `resolve_summary_template_prompt`). It replaces the final-pass system
/// prompt only: the map and intermediate merge prompts stay fixed so the
/// user-facing structure is rendered exactly once, by the terminal call.
pub async fn summarize_stream(
    transcript_text: &str,
    notes: Option<&str>,
    participants: &[MeetingParticipant],
    model: &str,
    ollama_base_url: Option<&str>,
    final_system_prompt: &str,
    on_chunk: impl Fn(SummarizeProgress),
) -> Result<String, String> {
    let provider = resolve_provider(model)?;
    let config = chunk_config(provider);
    let ollama_url = ollama_base_url.unwrap_or(OLLAMA_DEFAULT_URL);

    if estimate_tokens(transcript_text) <= config.stuff_token_limit {
        // Single-call path: the whole transcript fits, so this one call IS
        // the final pass and uses the template prompt.
        let full = generate_with_provider(
            provider,
            model,
            ollama_url,
            final_system_prompt,
            build_summarize_prompt(transcript_text, notes, participants),
            0.2,
            ollama::REDUCE_CONTEXT,
            &on_chunk,
            false,
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
                on_chunk(SummarizeProgress::stage_marker(
                    SummarizeStage::Map,
                    Some(i as u32 + 1),
                    Some(n as u32),
                ));
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
                    false,
                )
            })
            .buffered(config.map_concurrency)
            .try_collect()
            .await?;
    } else {
        for (i, chunk) in chunks.into_iter().enumerate() {
            on_chunk(SummarizeProgress::stage_marker(
                SummarizeStage::Map,
                Some(i as u32 + 1),
                Some(n as u32),
            ));
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
                false,
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
        final_system_prompt,
        &on_chunk,
    )
    .await?;
    Ok(sanitize_summary(&full))
}

/// Combine map-stage part summaries into the final prose summary.
///
/// Only the call that produces the truly final text uses the final,
/// user-facing system prompt ("## Summary / ## Topics"). Every intermediate
/// round, when a meeting needs more than one reduce pass to fit the
/// provider's context budget, uses the merge prompt instead, which stays a
/// flat unheaded bullet list. Without this split, each intermediate round
/// re-renders the full heading structure and the next round's merge just
/// concatenates already-headed blocks, so headings repeat once per surviving
/// branch of the reduce tree. The tree-walking control flow lives in
/// `run_reduce_tree` so it can be exercised with a canned generator in tests.
#[allow(clippy::too_many_arguments)]
async fn reduce_part_summaries(
    provider: SummaryProviderKind,
    model: &str,
    ollama_url: &str,
    part_summaries: &[String],
    notes: Option<&str>,
    participants: &[MeetingParticipant],
    config: ChunkConfig,
    final_system_prompt: &str,
    on_chunk: &impl Fn(SummarizeProgress),
) -> Result<String, String> {
    // The template prompt applies to the final pass only; intermediate
    // merge rounds keep the fixed provider merge prompt (both providers
    // share the same merge text today).
    let (merge_system_prompt, num_ctx) = match provider {
        SummaryProviderKind::Ollama => (ollama::MERGE_SYSTEM_PROMPT, ollama::REDUCE_CONTEXT),
        SummaryProviderKind::AppleIntelligence => {
            (apple::MERGE_SYSTEM_PROMPT, ollama::REDUCE_CONTEXT)
        }
    };

    on_chunk(SummarizeProgress::stage_marker(
        SummarizeStage::Combine,
        None,
        Some(part_summaries.len() as u32),
    ));

    let no_op = |_: SummarizeProgress| {};
    run_reduce_tree(
        part_summaries,
        notes,
        participants,
        config.reduce_token_limit,
        provider == SummaryProviderKind::Ollama,
        &|current, total| {
            on_chunk(SummarizeProgress::stage_marker(
                SummarizeStage::Combine,
                Some(current),
                Some(total),
            ));
        },
        |is_final, prompt| async move {
            let system =
                reduce_call_system_prompt(is_final, final_system_prompt, merge_system_prompt);
            if is_final {
                generate_with_provider(
                    provider, model, ollama_url, system, prompt, 0.3, num_ctx, on_chunk, false,
                )
                .await
            } else {
                generate_with_provider(
                    provider, model, ollama_url, system, prompt, 0.2, num_ctx, &no_op, false,
                )
                .await
            }
        },
    )
    .await
}

/// System prompt for one reduce-tree call: the user's summary template
/// controls the final pass only; every intermediate merge round keeps the
/// fixed, unheaded merge prompt.
fn reduce_call_system_prompt<'a>(
    is_final: bool,
    final_system_prompt: &'a str,
    merge_system_prompt: &'a str,
) -> &'a str {
    if is_final {
        final_system_prompt
    } else {
        merge_system_prompt
    }
}

/// Hierarchical merge-tree control flow, isolated from the concrete provider.
/// `generate(is_final, prompt)` performs one model call: `is_final` is true
/// exactly once, for the call whose output is returned to the caller; every
/// other call is an intermediate merge round. When `skip_initial_fit_check`
/// is true (Ollama, whose context window comfortably fits the whole reduce
/// prompt in practice) the first call is always the single final pass.
async fn run_reduce_tree<G, Fut>(
    part_summaries: &[String],
    notes: Option<&str>,
    participants: &[MeetingParticipant],
    reduce_token_limit: usize,
    skip_initial_fit_check: bool,
    on_batch_start: &impl Fn(u32, u32),
    mut generate: G,
) -> Result<String, String>
where
    G: FnMut(bool, String) -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    let mut token_limit = reduce_token_limit;
    // The token budget is an ESTIMATE (estimate_tokens); the real
    // FoundationModels tokenizer can exceed it and reject a prompt with a
    // context-overflow error even though the fit check passed. One structural
    // recovery is allowed per reduce: halve the budget and re-batch. Ollama
    // never emits the overflow marker, so its path is unaffected.
    let mut overflow_shrink_available = !skip_initial_fit_check;

    let mut current: Vec<String> = part_summaries.to_vec();
    'rounds: loop {
        if skip_initial_fit_check
            || reduce::reduce_prompt_fits(&current, notes, participants, token_limit)
        {
            match generate(true, build_reduce_prompt(&current, notes, participants)).await {
                Ok(text) => return Ok(text),
                Err(error)
                    if overflow_shrink_available
                        && current.len() > 1
                        && apple::is_context_overflow(&error) =>
                {
                    overflow_shrink_available = false;
                    token_limit /= 2;
                    continue 'rounds;
                }
                Err(error) => return Err(error),
            }
        }

        let batches = reduce::partition_for_reduce(&current, token_limit)?;
        if batches.len() == 1 && batches[0].len() == current.len() {
            let tokens = estimate_tokens(&build_reduce_prompt(&current, None, &[]));
            return Err(format!(
                "Meeting summary is too large for Apple Intelligence after batching \
                 ({tokens} estimated tokens, limit {token_limit}). Use Ollama for very long meetings."
            ));
        }

        let total_batches = batches.len() as u32;
        let mut next = Vec::with_capacity(batches.len());
        for (i, batch) in batches.into_iter().enumerate() {
            on_batch_start(i as u32 + 1, total_batches);
            match generate(false, build_reduce_prompt(&batch, None, &[])).await {
                Ok(merged) => next.push(merged),
                Err(error)
                    if overflow_shrink_available && apple::is_context_overflow(&error) =>
                {
                    // Redo the whole round with the tighter budget; merges
                    // already done this round are discarded, which is the
                    // price of a one-time recovery.
                    overflow_shrink_available = false;
                    token_limit /= 2;
                    continue 'rounds;
                }
                Err(error) => return Err(error),
            }
        }
        current = next;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        APPLE_INTELLIGENCE_MODEL_ID, ChunkConfig, SummaryProviderKind, apple, check_providers,
        ollama, reduce_call_system_prompt, resolve_provider, run_reduce_tree, sanitize_summary,
        structured_extract_for_persist,
    };
    use crate::transcript::StructuredSummary;

    #[test]
    fn structured_extract_for_persist_ok() {
        let structured = StructuredSummary {
            decisions: vec!["Ship".to_string()],
            action_items: vec![],
            open_questions: vec![],
        };
        let (persisted, warning) =
            structured_extract_for_persist(Ok(structured.clone()));
        assert_eq!(persisted, Some(structured));
        assert!(warning.is_none());
    }

    #[test]
    fn structured_extract_for_persist_err_falls_back_to_prose_only() {
        let (persisted, warning) =
            structured_extract_for_persist(Err("Parse structured summary JSON: eof".into()));
        assert!(persisted.is_none());
        assert_eq!(
            warning.as_deref(),
            Some("Parse structured summary JSON: eof")
        );
    }

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

    // --- Prompt builder heading policy ---------------------------------
    //
    // Root cause of the repeated-sections bug: the map stage and every
    // intermediate reduce round used to share the same system prompt as the
    // terminal pass (OLLAMA_SUMMARIZE_PROMPT), which demands the full
    // "## Summary / ## Decisions / ## Action Items / ## Topics" skeleton.
    // Concatenating several already-headed blocks in the next round repeated
    // those headings once per surviving branch of the reduce tree. The fix
    // keeps map and intermediate merge output as a flat, unheaded bullet
    // list; only the terminal pass may emit "## " headings, and it does so
    // exactly once each.

    #[test]
    fn map_prompt_has_no_markdown_headings() {
        assert!(
            !ollama::MAP_SYSTEM_PROMPT.contains("## "),
            "map stage must not request markdown headings: they repeat once chunks are merged"
        );
        assert_eq!(ollama::MAP_SYSTEM_PROMPT, apple::MAP_SYSTEM_PROMPT);
    }

    #[test]
    fn merge_prompt_has_no_markdown_headings() {
        assert!(
            !ollama::MERGE_SYSTEM_PROMPT.contains("## "),
            "intermediate reduce rounds must not request markdown headings: they repeat by the final round"
        );
        assert_eq!(ollama::MERGE_SYSTEM_PROMPT, apple::MERGE_SYSTEM_PROMPT);
    }

    #[test]
    fn final_summarize_prompt_renders_each_heading_exactly_once() {
        // The shipped default template IS the historical final-pass prompt;
        // both providers receive whatever template the user picked, so the
        // policy is checked on the constant itself.
        let prompt = crate::constants::OLLAMA_SUMMARIZE_PROMPT;
        assert_eq!(prompt.matches("## Summary").count(), 1);
        assert_eq!(prompt.matches("## Topics").count(), 1);
        // Decisions/action items/open questions are extracted separately
        // (structured_extract_for_persist -> Outcomes UI section); the prose
        // pass must not duplicate them.
        assert!(!prompt.contains("## Decisions"));
        assert!(!prompt.contains("## Action Items"));
        assert_eq!(
            super::default_summary_templates()[0].prompt,
            prompt,
            "the Default template must ship the standard final-pass prompt"
        );
    }

    // --- Simulated multi-round reduce -----------------------------------

    /// A part summary large enough that ~15 of them exceed Apple
    /// Intelligence's reduce budget, forcing `run_reduce_tree` into more
    /// than one hierarchical merge round (mirrors reduce::tests sizing).
    fn simulated_part(index: usize) -> String {
        format!(
            "Part {index} facts: {}",
            "decision action item discussion topic detail ".repeat(60)
        )
    }

    #[tokio::test]
    async fn run_reduce_tree_hierarchical_merge_renders_final_headers_once() {
        let parts: Vec<String> = (1..=40).map(simulated_part).collect();
        let reduce_token_limit = ChunkConfig::APPLE_INTELLIGENCE.reduce_token_limit;

        let final_calls = std::cell::Cell::new(0u32);
        let merge_calls = std::cell::Cell::new(0u32);

        // Fake "model": obeys whichever prompt style it's asked for, the same
        // way a real map-reduce round would. Intermediate rounds echo a flat,
        // unheaded merge note; the terminal round alone renders the
        // user-facing skeleton, proving headings survive at most once.
        let result = run_reduce_tree(
            &parts,
            None,
            &[],
            reduce_token_limit,
            false,
            &|_current, _total| {},
            |is_final, prompt| {
                let merged_count = prompt.matches("=== Part").count();
                if is_final {
                    final_calls.set(final_calls.get() + 1);
                } else {
                    merge_calls.set(merge_calls.get() + 1);
                }
                async move {
                    if is_final {
                        Ok(format!(
                            "## Summary\n- merged {merged_count} parts\n\n## Topics\n- assorted topics\n"
                        ))
                    } else {
                        Ok(format!("- merged note covering {merged_count} parts\n"))
                    }
                }
            },
        )
        .await
        .expect("reduce tree should succeed");

        assert!(
            merge_calls.get() >= 1,
            "expected at least one intermediate merge round for 40 oversized parts, got {}",
            merge_calls.get()
        );
        assert_eq!(final_calls.get(), 1, "exactly one call must be the final pass");
        assert_eq!(result.matches("## Summary").count(), 1);
        assert_eq!(result.matches("## Topics").count(), 1);
    }

    #[tokio::test]
    async fn run_reduce_tree_single_round_still_calls_final_once() {
        // Small input that fits in one reduce call: no intermediate rounds
        // needed, but the terminal call must still happen exactly once.
        let parts = vec!["- one fact".to_string(), "- another fact".to_string()];
        let final_calls = std::cell::Cell::new(0u32);

        let result = run_reduce_tree(
            &parts,
            None,
            &[],
            ChunkConfig::APPLE_INTELLIGENCE.reduce_token_limit,
            false,
            &|_current, _total| {},
            |is_final, _prompt| {
                assert!(is_final, "the only call for a small input must be final");
                final_calls.set(final_calls.get() + 1);
                async move { Ok("## Summary\n- one fact\n- another fact\n\n## Topics\n- none\n".to_string()) }
            },
        )
        .await
        .expect("reduce tree should succeed");

        assert_eq!(final_calls.get(), 1);
        assert_eq!(result.matches("## Summary").count(), 1);
    }

    // --- Context-overflow recovery ---------------------------------------

    /// Parts sized so the reduce prompt fits the full Apple budget (3200)
    /// but not half of it: after one overflow shrink the tree must re-batch
    /// instead of retrying the same oversized final prompt.
    fn overflow_test_parts() -> Vec<String> {
        (1..=6)
            .map(|i| format!("Part {i}: {}", "decision topic detail ".repeat(80)))
            .collect()
    }

    #[tokio::test]
    async fn run_reduce_tree_shrinks_budget_once_on_context_overflow() {
        let parts = overflow_test_parts();
        let limit = ChunkConfig::APPLE_INTELLIGENCE.reduce_token_limit;
        // Sanity: fits the full budget, does not fit half of it.
        assert!(super::reduce::reduce_prompt_fits(&parts, None, &[], limit));
        assert!(!super::reduce::reduce_prompt_fits(&parts, None, &[], limit / 2));

        let final_calls = std::cell::Cell::new(0u32);
        let merge_calls = std::cell::Cell::new(0u32);

        let result = run_reduce_tree(
            &parts,
            None,
            &[],
            limit,
            false,
            &|_current, _total| {},
            |is_final, _prompt| {
                if is_final {
                    final_calls.set(final_calls.get() + 1);
                } else {
                    merge_calls.set(merge_calls.get() + 1);
                }
                let first_final = is_final && final_calls.get() == 1;
                async move {
                    if first_final {
                        // The estimate said it fits; the real tokenizer says no.
                        Err("exceeded_context_window: prompt exceeds 4096 tokens".to_string())
                    } else if is_final {
                        Ok("## Summary\n- recovered\n".to_string())
                    } else {
                        Ok("- merged note\n".to_string())
                    }
                }
            },
        )
        .await
        .expect("reduce tree should recover from one context overflow");

        assert_eq!(result, "## Summary\n- recovered\n");
        assert_eq!(final_calls.get(), 2, "overflowed final + recovered final");
        assert!(
            merge_calls.get() >= 1,
            "the halved budget must force at least one merge round"
        );
    }

    #[tokio::test]
    async fn run_reduce_tree_gives_up_after_second_context_overflow() {
        let parts = overflow_test_parts();
        let error = run_reduce_tree(
            &parts,
            None,
            &[],
            ChunkConfig::APPLE_INTELLIGENCE.reduce_token_limit,
            false,
            &|_current, _total| {},
            |_is_final, _prompt| async move {
                Err::<String, _>("exceeded_context_window: still too large".to_string())
            },
        )
        .await
        .unwrap_err();
        assert!(error.contains("exceeded_context_window"), "got: {error}");
    }

    #[tokio::test]
    async fn run_reduce_tree_skip_fit_check_propagates_overflow_untouched() {
        // Ollama path (skip_initial_fit_check): no shrink recovery, the error
        // surfaces as-is from the single final call.
        let parts = vec!["- a fact".to_string()];
        let final_calls = std::cell::Cell::new(0u32);
        let error = run_reduce_tree(
            &parts,
            None,
            &[],
            ChunkConfig::OLLAMA.reduce_token_limit,
            true,
            &|_current, _total| {},
            |_is_final, _prompt| {
                final_calls.set(final_calls.get() + 1);
                async move { Err::<String, _>("exceeded_context_window: whatever".to_string()) }
            },
        )
        .await
        .unwrap_err();
        assert_eq!(final_calls.get(), 1);
        assert!(error.contains("exceeded_context_window"));
    }

    /// A user-selected summary template must reach ONLY the terminal
    /// `is_final` call of the reduce tree; every intermediate merge round
    /// keeps the fixed merge prompt. Uses the same system-prompt selection
    /// (`reduce_call_system_prompt`) as `reduce_part_summaries`.
    #[tokio::test]
    async fn run_reduce_tree_custom_template_reaches_only_the_final_call() {
        let parts: Vec<String> = (1..=40).map(simulated_part).collect();
        let custom_template = "You render minutes in the user's custom format.";

        let custom_calls = std::cell::Cell::new(0u32);
        let merge_prompt_calls = std::cell::Cell::new(0u32);

        run_reduce_tree(
            &parts,
            None,
            &[],
            ChunkConfig::APPLE_INTELLIGENCE.reduce_token_limit,
            false,
            &|_current, _total| {},
            |is_final, _prompt| {
                let system = reduce_call_system_prompt(
                    is_final,
                    custom_template,
                    ollama::MERGE_SYSTEM_PROMPT,
                );
                if system == custom_template {
                    assert!(is_final, "template prompt must never reach a merge round");
                    custom_calls.set(custom_calls.get() + 1);
                } else {
                    assert_eq!(system, ollama::MERGE_SYSTEM_PROMPT);
                    merge_prompt_calls.set(merge_prompt_calls.get() + 1);
                }
                async move { Ok("- merged\n".to_string()) }
            },
        )
        .await
        .expect("reduce tree should succeed");

        assert_eq!(
            custom_calls.get(),
            1,
            "the custom template must be used exactly once, on the final pass"
        );
        assert!(merge_prompt_calls.get() >= 1);
    }
}
