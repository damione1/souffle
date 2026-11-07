//! Opt-in, task-scoped benchmark diagnostics. No recording outside a measured run.
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Map,
    Merge,
    Final,
    Extract,
}

#[derive(Debug, Clone, Serialize)]
pub struct Call {
    pub phase: Phase,
    pub provider: String,
    pub temperature: f32,
    pub num_ctx: Option<u32>,
    pub num_predict: Option<u32>,
    pub prompt_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub elapsed_ms: u128,
    pub error: Option<String>,
    pub completed: bool,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Metrics {
    pub calls: Vec<Call>,
    pub map_retries: usize,
    pub map_splits: usize,
    pub merge_rounds: usize,
    pub maps: Vec<(usize, String)>,
}

type Recorder = Arc<Mutex<Metrics>>;
tokio::task_local! { static RUN: Recorder; static PHASE: Phase; }

pub async fn measure<F: Future>(future: F) -> (F::Output, Metrics) {
    let recorder = Arc::new(Mutex::new(Metrics::default()));
    let result = RUN.scope(recorder.clone(), future).await;
    let metrics = recorder.lock().expect("benchmark metrics poisoned").clone();
    (result, metrics)
}

pub(crate) async fn phase<F: Future>(phase: Phase, future: F) -> F::Output {
    PHASE.scope(phase, future).await
}

#[derive(Clone)]
pub(crate) struct Context {
    recorder: Recorder,
    phase: Phase,
}
pub(crate) fn context() -> Option<Context> {
    Some(Context {
        recorder: RUN.try_with(Clone::clone).ok()?,
        phase: PHASE.try_with(|p| *p).unwrap_or(Phase::Final),
    })
}

pub(crate) struct CallGuard {
    recorder: Recorder,
    index: usize,
    started: Instant,
}
impl Context {
    pub fn start(
        &self,
        provider: &str,
        temperature: f32,
        num_ctx: Option<u32>,
        num_predict: Option<u32>,
    ) -> CallGuard {
        let mut metrics = self.recorder.lock().expect("benchmark metrics poisoned");
        let index = metrics.calls.len();
        metrics.calls.push(Call {
            phase: self.phase,
            provider: provider.into(),
            temperature,
            num_ctx,
            num_predict,
            prompt_tokens: None,
            output_tokens: None,
            elapsed_ms: 0,
            error: None,
            completed: false,
        });
        CallGuard {
            recorder: self.recorder.clone(),
            index,
            started: Instant::now(),
        }
    }
}
impl CallGuard {
    pub fn tokens(&self, prompt: Option<u32>, output: Option<u32>) {
        let mut metrics = self.recorder.lock().expect("benchmark metrics poisoned");
        let call = &mut metrics.calls[self.index];
        call.prompt_tokens = prompt;
        call.output_tokens = output;
    }
    pub fn finish<T>(&self, result: &Result<T, String>) {
        let mut metrics = self.recorder.lock().expect("benchmark metrics poisoned");
        let call = &mut metrics.calls[self.index];
        call.elapsed_ms = self.started.elapsed().as_millis();
        call.completed = true;
        call.error = result.as_ref().err().cloned();
    }
}
impl Drop for CallGuard {
    fn drop(&mut self) {
        let mut metrics = self.recorder.lock().expect("benchmark metrics poisoned");
        let call = &mut metrics.calls[self.index];
        if !call.completed {
            call.elapsed_ms = self.started.elapsed().as_millis();
            call.error = Some("request failed, cancelled or timed out before completion".into());
        }
    }
}
pub(crate) fn update(f: impl FnOnce(&mut Metrics)) {
    if let Ok(recorder) = RUN.try_with(Clone::clone) {
        f(&mut recorder.lock().expect("benchmark metrics poisoned"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn abandoned_call_keeps_incomplete_fallback() {
        let (_, metrics) = measure(async {
            let _guard = context().unwrap().start("synthetic", 0.2, None, None);
        })
        .await;
        assert_eq!(metrics.calls.len(), 1);
        assert!(!metrics.calls[0].completed);
        assert_eq!(
            metrics.calls[0].error.as_deref(),
            Some("request failed, cancelled or timed out before completion")
        );
    }
    #[tokio::test]
    async fn measured_calls_preserve_absent_tokens_errors_and_phase() {
        let (_, metrics) = measure(async {
            phase(Phase::Map, async {
                let guard = context().unwrap().start("synthetic", 0.2, None, None);
                guard.finish(&Ok::<_, String>("facts"));
            })
            .await;
            phase(Phase::Extract, async {
                let guard = context()
                    .unwrap()
                    .start("synthetic", 0.1, Some(42), Some(7));
                guard.tokens(Some(13), Some(5));
                guard.finish(&Err::<(), _>("provider failed".to_string()));
            })
            .await;
            update(|m| {
                m.map_retries += 1;
                m.map_splits += 1;
                m.merge_rounds += 1;
            });
        })
        .await;
        assert_eq!(metrics.calls.len(), 2);
        assert_eq!(metrics.calls[0].phase, Phase::Map);
        assert_eq!(metrics.calls[0].prompt_tokens, None);
        assert_eq!(metrics.calls[1].phase, Phase::Extract);
        assert_eq!(metrics.calls[1].prompt_tokens, Some(13));
        assert_eq!(metrics.calls[1].error.as_deref(), Some("provider failed"));
        assert_eq!(
            (
                metrics.map_retries,
                metrics.map_splits,
                metrics.merge_rounds
            ),
            (1, 1, 1)
        );
        assert!(context().is_none());
    }
}
