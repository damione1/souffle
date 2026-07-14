use std::sync::mpsc;
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

/// Hard wall-clock budget for ONE FoundationModels request. The FFI call is
/// blocking and non-cancellable from Rust, so each request runs on its own
/// thread and the caller stops waiting after this long. The Swift bridge
/// enforces its own shorter timeout (see swift/apple_intelligence.swift) so
/// the request thread normally exits on its own; this is the backstop for a
/// bridge that wedges before even reaching its semaphore wait.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Pause before the single retry of a failed (non-timeout) request, so a
/// transient condition such as `rate_limited` has a chance to clear.
pub const RETRY_BACKOFF: Duration = Duration::from_secs(2);

/// Attempts per request: the original call plus one retry.
pub const MAX_ATTEMPTS: u32 = 2;

// Stable machine-readable markers produced by the Swift bridge
// (classifyGenerationError in swift/apple_intelligence.swift).
const CONTEXT_OVERFLOW_MARKER: &str = "exceeded_context_window";
const GUARDRAIL_MARKER: &str = "guardrail_violation";
const UNSUPPORTED_LANGUAGE_MARKER: &str = "unsupported_language";

pub fn validate_availability() -> Result<(), String> {
    if apple_intelligence::check_apple_intelligence_availability() {
        Ok(())
    } else {
        Err("Apple Intelligence is not available on this device.".into())
    }
}

/// True when the provider rejected the request because the prompt exceeded
/// the FoundationModels context window. The reduce tree treats this as a
/// signal that the token estimate undershot the real tokenizer and re-batches
/// with a smaller budget instead of retrying the same oversized prompt.
pub(crate) fn is_context_overflow(error: &str) -> bool {
    error.contains(CONTEXT_OVERFLOW_MARKER)
}

/// Deterministic failures repeat on an identical retry; everything else
/// (timeouts, rate limiting, transient model/asset hiccups) gets one retry.
fn is_retryable(error: &str) -> bool {
    !(error.contains(GUARDRAIL_MARKER)
        || error.contains(UNSUPPORTED_LANGUAGE_MARKER)
        || is_context_overflow(error))
}

enum AttemptOutcome {
    Completed(Result<String, String>),
    TimedOut,
}

/// Run one blocking call on a dedicated thread and wait at most `timeout`.
///
/// On timeout the thread is abandoned: it keeps running (one leaked thread
/// plus one in-flight FoundationModels request) until the call returns, then
/// its `send` fails against the dropped receiver and it exits. The handoff is
/// channel-owned and the closure captures only its own data, so an abandoned
/// call can never touch shared state.
fn run_attempt<F>(call: F, timeout: Duration) -> AttemptOutcome
where
    F: FnOnce() -> Result<String, String> + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("apple-ai-request".into())
        .spawn(move || {
            let _ = tx.send(call());
        });
    if spawned.is_err() {
        return AttemptOutcome::Completed(Err(
            "Failed to spawn Apple Intelligence request thread".into()
        ));
    }
    match rx.recv_timeout(timeout) {
        Ok(result) => AttemptOutcome::Completed(result),
        Err(_) => AttemptOutcome::TimedOut,
    }
}

/// Run one FoundationModels request with a hard timeout and a single retry.
///
/// `make_attempt` builds a fresh blocking call per attempt (the closure moves
/// onto the attempt thread, so a retry needs its own copy). Retryable
/// failures and timeouts are attempted `MAX_ATTEMPTS` times in total; the
/// last error is returned so the pipeline fails cleanly instead of hanging.
pub(crate) fn generate_guarded<C, F>(
    mut make_attempt: C,
    timeout: Duration,
    retry_backoff: Duration,
) -> Result<String, String>
where
    C: FnMut() -> F,
    F: FnOnce() -> Result<String, String> + Send + 'static,
{
    let mut last_error = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        match run_attempt(make_attempt(), timeout) {
            AttemptOutcome::Completed(Ok(text)) => return Ok(text),
            AttemptOutcome::Completed(Err(error)) => {
                if !is_retryable(&error) {
                    return Err(error);
                }
                tracing::warn!(attempt, error = %error, "Apple Intelligence request failed");
                last_error = error;
                if attempt < MAX_ATTEMPTS && !retry_backoff.is_zero() {
                    std::thread::sleep(retry_backoff);
                }
            }
            AttemptOutcome::TimedOut => {
                tracing::warn!(
                    attempt,
                    timeout_secs = timeout.as_secs(),
                    "Apple Intelligence request timed out; request thread abandoned"
                );
                last_error = format!(
                    "Apple Intelligence request timed out after {}s",
                    timeout.as_secs()
                );
                // No backoff after a timeout: the wall-clock budget is spent.
            }
        }
    }
    Err(last_error)
}

#[cfg(test)]
mod tests {
    use super::{generate_guarded, is_context_overflow, is_retryable};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    const SHORT_TIMEOUT: Duration = Duration::from_millis(50);
    const NO_BACKOFF: Duration = Duration::ZERO;

    #[test]
    fn success_propagates_on_first_attempt() {
        let attempts = Arc::new(AtomicU32::new(0));
        let counter = attempts.clone();
        let result = generate_guarded(
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
                || Ok("summary".to_string())
            },
            SHORT_TIMEOUT,
            NO_BACKOFF,
        );
        assert_eq!(result.as_deref(), Ok("summary"));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn timeout_then_success_retries_once_and_discards_late_result() {
        let attempts = Arc::new(AtomicU32::new(0));
        let counter = attempts.clone();
        let result = generate_guarded(
            move || {
                let attempt = counter.fetch_add(1, Ordering::SeqCst) + 1;
                move || {
                    if attempt == 1 {
                        // Outlives the timeout: the late Ok must be discarded
                        // (its channel receiver is gone) and never returned.
                        std::thread::sleep(Duration::from_millis(200));
                        Ok("late result from abandoned thread".to_string())
                    } else {
                        Ok("fresh result".to_string())
                    }
                }
            },
            SHORT_TIMEOUT,
            NO_BACKOFF,
        );
        assert_eq!(result.as_deref(), Ok("fresh result"));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        // Let the abandoned thread finish its doomed send; nothing to assert
        // beyond not panicking or corrupting the returned value.
        std::thread::sleep(Duration::from_millis(250));
    }

    #[test]
    fn two_timeouts_fail_with_timeout_error() {
        let attempts = Arc::new(AtomicU32::new(0));
        let counter = attempts.clone();
        let result = generate_guarded(
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
                || {
                    std::thread::sleep(Duration::from_millis(200));
                    Ok("never delivered".to_string())
                }
            },
            SHORT_TIMEOUT,
            NO_BACKOFF,
        );
        let error = result.unwrap_err();
        assert!(error.contains("timed out"), "unexpected error: {error}");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        std::thread::sleep(Duration::from_millis(400));
    }

    #[test]
    fn transient_error_retries_then_fails_with_last_error() {
        let attempts = Arc::new(AtomicU32::new(0));
        let counter = attempts.clone();
        let result = generate_guarded(
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
                || Err("rate_limited: system is busy".to_string())
            },
            SHORT_TIMEOUT,
            NO_BACKOFF,
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
        ] {
            let attempts = Arc::new(AtomicU32::new(0));
            let counter = attempts.clone();
            let result = generate_guarded(
                move || {
                    counter.fetch_add(1, Ordering::SeqCst);
                    move || Err(marker.to_string())
                },
                SHORT_TIMEOUT,
                NO_BACKOFF,
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
        assert!(is_retryable("rate_limited: busy"));
        assert!(is_retryable("some opaque transient failure"));
        assert!(!is_retryable("guardrail_violation: refused"));
        assert!(!is_retryable("exceeded_context_window: too big"));
    }
}
