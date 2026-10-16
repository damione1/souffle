use std::time::Duration;

use crate::apple_intelligence;
use crate::constants::{
    OLLAMA_DICTATION_POLISH_PROMPT, OLLAMA_MAP_PROMPT, OLLAMA_MERGE_PROMPT,
    OLLAMA_STRUCTURED_EXTRACT_PROMPT,
};

pub const MAP_SYSTEM_PROMPT: &str = OLLAMA_MAP_PROMPT;
pub const MERGE_SYSTEM_PROMPT: &str = OLLAMA_MERGE_PROMPT;
pub const STRUCTURED_EXTRACT_SYSTEM_PROMPT: &str = OLLAMA_STRUCTURED_EXTRACT_PROMPT;
pub const DICTATION_POLISH_SYSTEM_PROMPT: &str = OLLAMA_DICTATION_POLISH_PROMPT;

/// Hard wall-clock budget for ONE FoundationModels meeting summary / map /
/// reduce / extract request. Enforced directly by `run_helper_process` in
/// `apple_intelligence.rs`, which kills and reaps the helper subprocess on
/// expiry without leaking threads or orphaned processes.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Shorter wall-clock budget for latency-sensitive dictation polish requests.
pub const POLISH_TIMEOUT: Duration = Duration::from_secs(20);

/// Pause before the single retry of a failed (non-timeout) request, so a
/// transient condition such as `rate_limited` has a chance to clear.
pub const RETRY_BACKOFF: Duration = Duration::from_secs(2);

/// Attempts per request: the original call plus one retry.
pub const MAX_ATTEMPTS: u32 = 2;

/// Maximum explicit `maximumResponseTokens` cap passed to FoundationModels when
/// a stage specifies a bounded `num_predict` below the 4,096-token combined
/// context window. Stages whose `num_predict` is 4,096 or higher (e.g.
/// `REDUCE_BUDGET`, sized for Ollama's 16,384 window) pass `0` (`nil` in Swift)
/// so FoundationModels can use all remaining tokens in its 4,096-token window.
const MAX_EXPLICIT_RESPONSE_TOKENS: u32 = 1_200;

// Stable machine-readable markers produced by the Swift bridge
// (classifyGenerationError in swift/apple_intelligence.swift).
const CONTEXT_OVERFLOW_MARKER: &str = "exceeded_context_window";
const GUARDRAIL_MARKER: &str = "guardrail_violation";
const UNSUPPORTED_LANGUAGE_MARKER: &str = "unsupported_language";
const REQUEST_TIMEOUT_MARKER: &str = "request_timeout";

pub fn validate_availability() -> Result<(), String> {
    if apple_intelligence::check_apple_intelligence_availability() {
        Ok(())
    } else {
        Err("Apple Intelligence is not available on this device.".into())
    }
}

/// Select the per-attempt helper timeout for the given system prompt.
pub(crate) fn request_timeout_for(system_prompt: &str) -> Duration {
    if system_prompt == DICTATION_POLISH_SYSTEM_PROMPT {
        POLISH_TIMEOUT
    } else {
        REQUEST_TIMEOUT
    }
}

/// Map a stage [`super::ollama::GenerationBudget`] to the `max_tokens` parameter
/// sent to the FoundationModels bridge (`0` means `maximumResponseTokens: nil`,
/// letting FoundationModels use the remaining 4,096-token context window).
pub(crate) fn response_token_limit(budget: super::ollama::GenerationBudget) -> i32 {
    if budget.num_predict == 0 || budget.num_predict >= super::ollama::REDUCE_BUDGET.num_predict {
        0
    } else {
        budget.num_predict.min(MAX_EXPLICIT_RESPONSE_TOKENS) as i32
    }
}

/// True when the provider rejected the request because the prompt exceeded
/// the FoundationModels context window. The reduce tree treats this as a
/// signal that the token estimate undershot the real tokenizer and re-batches
/// with a smaller budget instead of retrying the same oversized prompt.
pub(crate) fn is_context_overflow(error: &str) -> bool {
    error.contains(CONTEXT_OVERFLOW_MARKER)
}

/// True when the provider rejected the request due to safety guardrails or
/// refusal. Used both to skip useless retries and to allow a multi-chunk map
/// pass to skip a single blocked chunk without aborting the entire meeting.
pub(crate) fn is_guardrail_violation(error: &str) -> bool {
    error.contains(GUARDRAIL_MARKER) || error.to_ascii_lowercase().contains("safety guardrails")
}

fn is_timeout_error(error: &str) -> bool {
    error.starts_with(apple_intelligence::HELPER_TIMEOUT_PREFIX)
        || error.starts_with(REQUEST_TIMEOUT_MARKER)
}

/// Deterministic failures repeat on an identical retry; everything else
/// (timeouts, rate limiting, transient model/asset hiccups) gets one retry.
fn is_retryable(error: &str) -> bool {
    !(apple_intelligence::is_terminal_helper_error(error)
        || is_guardrail_violation(error)
        || error.contains(UNSUPPORTED_LANGUAGE_MARKER)
        || is_context_overflow(error))
}

/// Run one FoundationModels request with a hard timeout and a single retry.
///
/// `make_attempt` receives `timeout` and delegates to `apple_intelligence::process_text_with_system_prompt`,
/// whose subprocess supervisor enforces the wall-clock deadline and reaps the
/// child process on timeout. Retryable failures and timeouts are attempted
/// `MAX_ATTEMPTS` times in total; the last error is returned so the pipeline
/// fails cleanly instead of hanging.
pub(crate) fn generate_guarded<C>(
    mut make_attempt: C,
    timeout: Duration,
    retry_backoff: Duration,
    measurement: Option<(&super::metrics::Context, f32)>,
) -> Result<String, String>
where
    C: FnMut(Duration) -> Result<String, String>,
{
    let mut last_error = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        let guard = measurement.map(|(context, temperature)| {
            context.start("apple_intelligence", temperature, None, None)
        });
        let result = make_attempt(timeout);
        if let Some(guard) = guard {
            guard.finish(&result);
        }
        match result {
            Ok(text) => return Ok(text),
            Err(error) => {
                if !is_retryable(&error) {
                    return Err(error);
                }
                if is_timeout_error(&error) {
                    tracing::warn!(
                        attempt,
                        timeout_secs = timeout.as_secs(),
                        error = %error,
                        "Apple Intelligence request timed out"
                    );
                    last_error = error;
                    // No backoff after a timeout: the attempt's wall-clock budget is spent.
                } else {
                    tracing::warn!(attempt, error = %error, "Apple Intelligence request failed");
                    last_error = error;
                    if attempt < MAX_ATTEMPTS && !retry_backoff.is_zero() {
                        std::thread::sleep(retry_backoff);
                    }
                }
            }
        }
    }
    Err(last_error)
}

#[cfg(test)]
mod tests {
    use super::{
        DICTATION_POLISH_SYSTEM_PROMPT, MAP_SYSTEM_PROMPT, POLISH_TIMEOUT, REQUEST_TIMEOUT,
        generate_guarded, is_context_overflow, is_retryable, request_timeout_for,
        response_token_limit,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    const SHORT_TIMEOUT: Duration = Duration::from_millis(50);
    const NO_BACKOFF: Duration = Duration::ZERO;

    #[tokio::test]
    async fn diagnostics_finalize_timeout_before_successful_retry() {
        let (result, metrics) = crate::summary::metrics::measure(async {
            let context = crate::summary::metrics::context().unwrap();
            let mut attempt = 0;
            generate_guarded(
                |timeout| {
                    attempt += 1;
                    if attempt == 1 {
                        std::thread::sleep(timeout);
                        Err(format!(
                            "{} exceeded {} ms",
                            crate::apple_intelligence::HELPER_TIMEOUT_PREFIX,
                            timeout.as_millis()
                        ))
                    } else {
                        Ok("summary".into())
                    }
                },
                SHORT_TIMEOUT,
                NO_BACKOFF,
                Some((&context, 0.2)),
            )
        })
        .await;
        assert_eq!(result.as_deref(), Ok("summary"));
        assert_eq!(metrics.calls.len(), 2);
        let timeout = &metrics.calls[0];
        assert!(timeout.completed);
        assert!(
            timeout
                .error
                .as_ref()
                .unwrap()
                .starts_with(crate::apple_intelligence::HELPER_TIMEOUT_PREFIX)
        );
        assert!(timeout.elapsed_ms >= SHORT_TIMEOUT.as_millis());
        assert_eq!(timeout.prompt_tokens, None);
        assert!(metrics.calls[1].completed);
        assert!(metrics.calls[1].error.is_none());
    }

    #[test]
    fn success_propagates_on_first_attempt() {
        let attempts = Arc::new(AtomicU32::new(0));
        let counter = attempts.clone();
        let result = generate_guarded(
            move |_timeout| {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok("summary".to_string())
            },
            SHORT_TIMEOUT,
            NO_BACKOFF,
            None,
        );
        assert_eq!(result.as_deref(), Ok("summary"));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn timeout_then_success_retries_once() {
        let attempts = Arc::new(AtomicU32::new(0));
        let counter = attempts.clone();
        let result = generate_guarded(
            move |timeout| {
                let attempt = counter.fetch_add(1, Ordering::SeqCst) + 1;
                if attempt == 1 {
                    Err(format!(
                        "{} exceeded {} ms",
                        crate::apple_intelligence::HELPER_TIMEOUT_PREFIX,
                        timeout.as_millis()
                    ))
                } else {
                    Ok("fresh result".to_string())
                }
            },
            SHORT_TIMEOUT,
            NO_BACKOFF,
            None,
        );
        assert_eq!(result.as_deref(), Ok("fresh result"));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn two_timeouts_fail_with_timeout_error() {
        let attempts = Arc::new(AtomicU32::new(0));
        let counter = attempts.clone();
        let result = generate_guarded(
            move |timeout| {
                counter.fetch_add(1, Ordering::SeqCst);
                Err(format!(
                    "{} exceeded {} ms",
                    crate::apple_intelligence::HELPER_TIMEOUT_PREFIX,
                    timeout.as_millis()
                ))
            },
            SHORT_TIMEOUT,
            NO_BACKOFF,
            None,
        );
        let error = result.unwrap_err();
        assert!(
            error.starts_with(crate::apple_intelligence::HELPER_TIMEOUT_PREFIX),
            "unexpected error: {error}"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn transient_error_retries_then_fails_with_last_error() {
        let attempts = Arc::new(AtomicU32::new(0));
        let counter = attempts.clone();
        let result = generate_guarded(
            move |_timeout| {
                counter.fetch_add(1, Ordering::SeqCst);
                Err("rate_limited: system is busy".to_string())
            },
            SHORT_TIMEOUT,
            NO_BACKOFF,
            None,
        );
        assert_eq!(result.unwrap_err(), "rate_limited: system is busy");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn non_retryable_error_fails_on_first_attempt() {
        for marker in [
            "guardrail_violation: safety filter",
            "unsupported_language: locale",
            "exceeded_context_window: 4096 tokens",
            crate::apple_intelligence::HELPER_PROCESS_FAILURE_PREFIX,
        ] {
            let attempts = Arc::new(AtomicU32::new(0));
            let counter = attempts.clone();
            let result = generate_guarded(
                move |_timeout| {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Err(marker.to_string())
                },
                SHORT_TIMEOUT,
                NO_BACKOFF,
                None,
            );
            assert_eq!(result.unwrap_err(), marker);
            assert_eq!(attempts.load(Ordering::SeqCst), 1, "marker: {marker}");
        }
    }

    #[test]
    fn error_classification_markers() {
        assert!(is_context_overflow("exceeded_context_window: too big"));
        assert!(!is_context_overflow("rate_limited: busy"));
        assert!(is_retryable("request_timeout: no answer in 100s"));
        assert!(is_retryable(
            crate::apple_intelligence::HELPER_TIMEOUT_PREFIX
        ));
        assert!(is_retryable("rate_limited: busy"));
        assert!(is_retryable("some opaque transient failure"));
        assert!(!is_retryable("guardrail_violation: refused"));
        assert!(!is_retryable("exceeded_context_window: too big"));
        assert!(!is_retryable(
            crate::apple_intelligence::HELPER_PROCESS_FAILURE_PREFIX
        ));
    }

    #[test]
    fn timeout_and_response_token_budgets_match_stage() {
        assert_eq!(
            request_timeout_for(DICTATION_POLISH_SYSTEM_PROMPT),
            POLISH_TIMEOUT
        );
        assert_eq!(request_timeout_for(MAP_SYSTEM_PROMPT), REQUEST_TIMEOUT);
        assert_eq!(
            response_token_limit(crate::summary::ollama::MAP_BUDGET),
            700
        );
        assert_eq!(
            response_token_limit(crate::summary::ollama::REDUCE_BUDGET),
            0
        );
    }
}
