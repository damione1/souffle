//! One engine join per process, with bounded waits for each Quit attempt.

use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError};

enum State {
    Idle,
    Waiting(Receiver<Result<(), String>>),
    Finished(Result<(), String>),
}

pub struct EngineShutdown {
    state: State,
}

impl EngineShutdown {
    pub const fn new() -> Self {
        Self { state: State::Idle }
    }

    /// A timeout leaves the same join in flight. Retrying Quit waits for it,
    /// never spawning another worker or claiming that native resources died.
    pub fn wait(
        &mut self,
        timeout: Duration,
        shutdown: impl FnOnce() -> Result<(), String> + Send + 'static,
    ) -> Result<(), String> {
        match &self.state {
            State::Idle => {
                let (tx, rx) = crossbeam_channel::bounded(1);
                std::thread::Builder::new()
                    .name("quit-engine-shutdown".into())
                    .spawn(move || {
                        let _ = tx.send(shutdown());
                    })
                    .map_err(|error| format!("Could not start engine shutdown: {error}"))?;
                self.state = State::Waiting(rx);
            }
            State::Waiting(_) | State::Finished(_) => {}
        }
        let result = match &self.state {
            State::Idle => unreachable!("shutdown worker was started above"),
            State::Waiting(rx) => match rx.recv_timeout(timeout) {
                Ok(result) => result,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(
                        "Could not quit: the transcription engine is still shutting down. Try Quit again after it finishes; if it remains blocked, use Force Quit."
                            .into(),
                    );
                }
                Err(RecvTimeoutError::Disconnected) => {
                    Err("Engine shutdown worker disconnected before confirming cleanup".into())
                }
            },
            State::Finished(result) => return result.clone(),
        };
        self.state = State::Finished(result.clone());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_out_quit_reuses_join_and_waits_for_actual_completion() {
        let (entered_tx, entered_rx) = crossbeam_channel::bounded(1);
        let (release_tx, release_rx) = crossbeam_channel::bounded(1);
        let mut shutdown = EngineShutdown::new();
        assert!(
            shutdown
                .wait(Duration::from_millis(20), move || {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap_err()
                .contains("still shutting down")
        );
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(
            shutdown
                .wait(Duration::from_millis(20), || panic!(
                    "a second join was started"
                ))
                .is_err()
        );
        release_tx.send(()).unwrap();
        shutdown
            .wait(Duration::from_secs(1), || {
                panic!("a third join was started")
            })
            .unwrap();
        shutdown
            .wait(Duration::ZERO, || panic!("completed shutdown restarted"))
            .unwrap();
    }

    #[test]
    fn failed_cleanup_never_becomes_success_on_retry() {
        let mut shutdown = EngineShutdown::new();
        let error = "native cleanup failed";
        assert_eq!(
            shutdown.wait(Duration::from_secs(1), move || Err(error.into())),
            Err(error.into())
        );
        assert_eq!(
            shutdown.wait(Duration::ZERO, || panic!("failed join restarted")),
            Err(error.into())
        );
    }
}
