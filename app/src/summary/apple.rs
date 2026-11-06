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

/// Pause before retrying a transient failure other than a timeout, so a
/// transient condition such as `rate_limited` has a chance to clear.
pub const RETRY_BACKOFF: Duration = Duration::from_secs(2);

/// Attempts per request: the original call plus one retry.
pub const MAX_ATTEMPTS: u32 = 2;

/// The caller declares the purpose; prompt wording and translations cannot
/// change the wall-clock budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestPurpose {
    DictationPolish,
    MeetingSummary,
}

impl RequestPurpose {
    pub(crate) fn timeout(self) -> Duration {
        match self {
            Self::DictationPolish => POLISH_TIMEOUT,
            Self::MeetingSummary => REQUEST_TIMEOUT,
        }
    }
}

pub fn validate_availability() -> Result<(), String> {
    if apple_intelligence::check_apple_intelligence_availability() {
        Ok(())
    } else {
        Err("Apple Intelligence is not available on this device.".into())
    }
}

/// `0` maps to `maximumResponseTokens: nil` in Swift. The final summary
/// retains the full remaining context, while bounded stages use their budget.
pub(crate) fn response_token_limit(budget: super::ollama::GenerationBudget) -> i32 {
    if budget.num_predict >= super::ollama::REDUCE_BUDGET.num_predict {
        0
    } else {
        budget.num_predict as i32
    }
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
) -> Result<String, apple_intelligence::AppleIntelligenceError>
where
    C: FnMut(Duration) -> Result<String, apple_intelligence::AppleIntelligenceError>,
{
    let mut last_error = None;
    for attempt in 1..=MAX_ATTEMPTS {
        let guard = measurement.map(|(context, temperature)| {
            context.start("apple_intelligence", temperature, None, None)
        });
        let result = make_attempt(timeout);
        if let Some(guard) = guard {
            guard.finish(&result.as_ref().map(|_| ()).map_err(ToString::to_string));
        }
        match result {
            Ok(text) => return Ok(text),
            Err(error) => {
                if !error.is_retryable() {
                    tracing::warn!(
                        attempt,
                        error = %error,
                        "Apple Intelligence request failed with non-retryable error"
                    );
                    return Err(error);
                }
                match &error {
                    apple_intelligence::AppleIntelligenceError::Timeout(_) => {
                        tracing::warn!(
                            attempt,
                            timeout_secs = timeout.as_secs(),
                            error = %error,
                            "Apple Intelligence request timed out"
                        );
                        last_error = Some(error);
                        // No backoff after a timeout: the attempt's wall-clock budget is spent.
                    }
                    apple_intelligence::AppleIntelligenceError::ProcessFailure(_)
                    | apple_intelligence::AppleIntelligenceError::FatalBridge(_)
                    | apple_intelligence::AppleIntelligenceError::ContextOverflow(_)
                    | apple_intelligence::AppleIntelligenceError::GuardrailViolation(_)
                    | apple_intelligence::AppleIntelligenceError::UnsupportedLanguage(_)
                    | apple_intelligence::AppleIntelligenceError::Provider(_) => {
                        tracing::warn!(attempt, error = %error, "Apple Intelligence request failed");
                        last_error = Some(error);
                        if attempt < MAX_ATTEMPTS && !retry_backoff.is_zero() {
                            std::thread::sleep(retry_backoff);
                        }
                    }
                }
            }
        }
    }
    Err(last_error.expect("at least one generation attempt is configured"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use apple_intelligence::AppleIntelligenceError as Error;

    #[test]
    fn budgets_are_explicit_by_purpose() {
        assert_eq!(
            RequestPurpose::DictationPolish.timeout(),
            Duration::from_secs(20)
        );
        assert_eq!(
            RequestPurpose::MeetingSummary.timeout(),
            Duration::from_secs(120)
        );
        assert_eq!(response_token_limit(super::super::ollama::MAP_BUDGET), 700);
        assert_eq!(response_token_limit(super::super::ollama::REDUCE_BUDGET), 0);
        assert_eq!(
            response_token_limit(super::super::ollama::GenerationBudget {
                num_ctx: 4096,
                num_predict: 0,
            }),
            0
        );
    }

    #[tokio::test]
    async fn timeout_retry_records_failure_then_success() {
        let (result, metrics) = super::super::metrics::measure(async {
            let context = super::super::metrics::context().unwrap();
            let mut calls = 0;
            generate_guarded(
                |timeout| {
                    assert_eq!(timeout, RequestPurpose::MeetingSummary.timeout());
                    calls += 1;
                    if calls == 1 {
                        Err(Error::Timeout("expired".into()))
                    } else {
                        Ok("summary".into())
                    }
                },
                RequestPurpose::MeetingSummary.timeout(),
                Duration::ZERO,
                Some((&context, 0.2)),
            )
        })
        .await;
        assert_eq!(result.as_deref(), Ok("summary"));
        assert_eq!(metrics.calls.len(), 2);
        assert!(metrics.calls.iter().all(|call| call.completed));
        assert!(metrics.calls[0].error.is_some());
        assert!(metrics.calls[1].error.is_none());
    }

    #[test]
    fn deterministic_errors_are_not_retried() {
        for error in [
            Error::GuardrailViolation("refused".into()),
            Error::UnsupportedLanguage("locale".into()),
            Error::ContextOverflow("full".into()),
            Error::ProcessFailure("exit".into()),
            Error::FatalBridge("spawn".into()),
        ] {
            let mut calls = 0;
            let result = generate_guarded(
                |_| {
                    calls += 1;
                    Err(error.clone())
                },
                Duration::from_millis(40),
                Duration::ZERO,
                None,
            );
            assert_eq!(result.unwrap_err(), error);
            assert_eq!(calls, 1);
        }
    }

    #[test]
    fn transient_failure_returns_last_error_after_two_attempts() {
        let mut calls = 0;
        let result = generate_guarded(
            |_| {
                calls += 1;
                Err(Error::Provider(format!("rate limited {calls}")))
            },
            Duration::from_millis(40),
            Duration::ZERO,
            None,
        );
        assert_eq!(calls, MAX_ATTEMPTS);
        assert_eq!(
            result.unwrap_err(),
            Error::Provider("rate limited 2".into())
        );
    }
}
