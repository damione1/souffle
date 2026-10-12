//! Bounded control of a thread-confined AVAudioEngine. A wedged CoreAudio call
//! cannot be cancelled, so keep its owner until it really exits and refuse to
//! stack another engine behind it. Never move Native or join this thread.
use super::{Native, Sink, Telemetry, devices::Selection};
use serde_json::Value;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, String>;

type CheckReply = SyncSender<Result<Value>>;

struct Worker {
    commands: Option<SyncSender<CheckReply>>,
    cancelled: Arc<AtomicBool>,
    telemetry: Arc<Telemetry>,
    finished: Receiver<()>,
    thread: JoinHandle<()>,
}

impl Worker {
    fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.telemetry.accepting.store(false, Ordering::Release);
        self.commands.take();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        // Dropping JoinHandle detaches. Native, including a late result, stays
        // on its owner until the OS call returns; nothing joins a stuck call.
    }
}

pub(crate) struct Started {
    pub selection: Selection,
    pub rate: u32,
    pub log: Value,
}

#[derive(Default)]
pub(crate) struct Owner {
    worker: Option<Worker>,
}

impl Owner {
    pub fn start(
        &mut self,
        uid: String,
        producer: Sink,
        telemetry: Arc<Telemetry>,
        timeout: Duration,
    ) -> Result<Started> {
        let native_telemetry = Arc::clone(&telemetry);
        self.start_with(
            telemetry,
            timeout,
            move |cancelled| {
                let selection = Selection::for_input_uid(&uid)
                    .map_err(|e| format!("Apple microphone selection failed: {e}"))?;
                if cancelled.load(Ordering::Acquire) {
                    return Err("Apple microphone start cancelled during selection".into());
                }
                let mut log = serde_json::json!({});
                // Silence only: the system tap never feeds the native output.
                let result =
                    Native::start(producer, native_telemetry, None, Some(&selection), &mut log);
                tracing::info!("Apple meeting microphone requested={selection:?} readbacks={log}");
                let native = result.map_err(|e| format!("Apple meeting microphone failed: {e}"))?;
                let started = Started {
                    selection: selection.clone(),
                    rate: native.rate,
                    log: log.clone(),
                };
                Ok(((native, selection, log), started))
            },
            |(native, selection, log)| {
                native
                    .check_route(selection, log)
                    .map_err(|e| e.to_string())?;
                Ok(log.clone())
            },
        )
    }

    /// T is deliberately not Send: it is created, used and destroyed entirely
    /// inside the owner. The injected operations also exercise stuck drivers
    /// in deterministic tests without opening a physical audio device.
    fn start_with<T, I, F, C>(
        &mut self,
        telemetry: Arc<Telemetry>,
        timeout: Duration,
        start: F,
        mut check: C,
    ) -> Result<I>
    where
        I: Send + 'static,
        F: FnOnce(&AtomicBool) -> Result<(T, I)> + Send + 'static,
        C: FnMut(&mut T) -> Result<Value> + Send + 'static,
    {
        let begun = Instant::now();
        self.stop();
        if let Some(worker) = self.worker.as_ref()
            && !worker.thread.is_finished()
            && worker.finished.recv_timeout(timeout).is_err()
        {
            return Err(
                "Apple microphone unavailable: previous native owner is still stopping".into(),
            );
        }
        self.worker.take();

        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let worker_telemetry = Arc::clone(&telemetry);
        let (commands, commands_rx) = mpsc::sync_channel::<CheckReply>(1);
        let (ready, ready_rx) = mpsc::sync_channel(1);
        let (finished_tx, finished) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("apple-mic-owner".into())
            .spawn(move || {
                // Drop this only after the thread-confined value has dropped,
                // including during unwinding. A new owner cannot overlap it.
                struct Completion(SyncSender<()>);
                impl Drop for Completion {
                    fn drop(&mut self) {
                        let _ = self.0.try_send(());
                    }
                }
                let _completion = Completion(finished_tx);
                objc2::rc::autoreleasepool(|_| {
                    let (mut native, info) = match start(&worker_cancelled) {
                        Ok(value) => value,
                        Err(error) => {
                            let _ = ready.try_send(Err(error));
                            return;
                        }
                    };
                    if worker_cancelled.load(Ordering::Acquire) || ready.try_send(Ok(info)).is_err()
                    {
                        worker_telemetry.accepting.store(false, Ordering::Release);
                        return;
                    }
                    while let Ok(reply) = commands_rx.recv() {
                        if worker_cancelled.load(Ordering::Acquire) {
                            break;
                        }
                        let result = check(&mut native);
                        let _ = reply.try_send(result);
                    }
                    worker_telemetry.accepting.store(false, Ordering::Release);
                    // native drops here, on this thread, even after a timeout.
                });
            })
            .map_err(|e| format!("Could not spawn Apple microphone owner: {e}"))?;
        self.worker = Some(Worker {
            commands: Some(commands),
            cancelled,
            telemetry,
            finished,
            thread,
        });
        match ready_rx.recv_timeout(timeout.saturating_sub(begun.elapsed())) {
            Ok(Ok(info)) => Ok(info),
            Ok(Err(error)) => {
                self.stop();
                Err(error)
            }
            Err(error) => {
                self.stop();
                Err(format!(
                    "Apple microphone start did not complete within {timeout:?}: {error}"
                ))
            }
        }
    }

    pub fn check(&mut self, timeout: Duration) -> Result<Value> {
        let (reply, response) = mpsc::sync_channel(1);
        let result = (|| {
            let commands = self
                .worker
                .as_ref()
                .and_then(|worker| worker.commands.as_ref())
                .ok_or("Apple microphone owner is unavailable")?;
            commands
                .try_send(reply)
                .map_err(|e| format!("Apple microphone check could not be queued: {e}"))?;
            response.recv_timeout(timeout).map_err(|e| {
                format!("Apple microphone check did not complete within {timeout:?}: {e}")
            })?
        })();
        if result.is_err() {
            self.stop();
        }
        result
    }

    pub fn stop(&mut self) {
        if let Some(worker) = self.worker.as_mut() {
            worker.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;
    use std::thread::{self, ThreadId};

    const BOUND: Duration = Duration::from_millis(40);
    const DEADLINE: Duration = Duration::from_secs(2);

    // Rc makes this value !Send, just like the retained AVAudioEngine. Drop
    // reports its thread and whether delivery had already been disabled.
    struct Local {
        _not_send: Rc<()>,
        telemetry: Arc<Telemetry>,
        dropped: SyncSender<(ThreadId, bool)>,
        drop_gate: Option<Receiver<()>>,
        dropping: Option<SyncSender<()>>,
    }

    impl Local {
        fn new(telemetry: Arc<Telemetry>, dropped: SyncSender<(ThreadId, bool)>) -> Self {
            Self {
                _not_send: Rc::new(()),
                telemetry,
                dropped,
                drop_gate: None,
                dropping: None,
            }
        }
    }

    impl Drop for Local {
        fn drop(&mut self) {
            if let Some(dropping) = &self.dropping {
                dropping.try_send(()).unwrap();
            }
            if let Some(gate) = &self.drop_gate {
                gate.recv_timeout(DEADLINE).unwrap();
            }
            self.dropped
                .try_send((
                    thread::current().id(),
                    self.telemetry.accepting.load(Ordering::Acquire),
                ))
                .unwrap();
        }
    }

    fn refused_retry(owner: &mut Owner) {
        let (opened, observed) = mpsc::sync_channel(1);
        let error = owner
            .start_with(
                Arc::new(Telemetry::default()),
                BOUND,
                move |_| {
                    opened.try_send(()).unwrap();
                    Ok((Rc::new(()), ()))
                },
                |_| Ok(Value::Null),
            )
            .unwrap_err();
        assert!(error.contains("previous native owner is still stopping"));
        assert!(observed.try_recv().is_err(), "retry stacked another owner");
    }

    #[test]
    fn blocked_start_times_out_and_late_native_drops_on_its_owner_without_adoption() {
        let mut owner = Owner::default();
        let telemetry = Arc::new(Telemetry::default());
        let worker_telemetry = Arc::clone(&telemetry);
        let (release, gate) = mpsc::sync_channel(1);
        let (entered, entry) = mpsc::sync_channel(1);
        let (dropped, drop_report) = mpsc::sync_channel(1);
        let started = Instant::now();
        let error = owner
            .start_with(
                Arc::clone(&telemetry),
                BOUND,
                move |_| {
                    let id = thread::current().id();
                    entered.try_send(id).unwrap();
                    gate.recv_timeout(DEADLINE).unwrap();
                    Ok((Local::new(worker_telemetry, dropped), id))
                },
                |_| Ok(Value::Null),
            )
            .unwrap_err();
        assert!(error.contains("start did not complete"));
        assert!(started.elapsed() < DEADLINE);
        let owner_thread = entry.recv_timeout(DEADLINE).unwrap();
        assert_ne!(owner_thread, thread::current().id());
        assert!(!telemetry.accepting.load(Ordering::Acquire));
        refused_retry(&mut owner);
        release.send(()).unwrap();
        assert_eq!(
            drop_report.recv_timeout(DEADLINE).unwrap(),
            (owner_thread, false)
        );

        // A recovered driver permits a fresh session on the same controller.
        let (dropped, drop_report) = mpsc::sync_channel(1);
        let worker_telemetry = Arc::clone(&telemetry);
        let next_thread = owner
            .start_with(
                Arc::clone(&telemetry),
                DEADLINE,
                move |_| {
                    Ok((
                        Local::new(worker_telemetry, dropped),
                        thread::current().id(),
                    ))
                },
                |_| Ok(serde_json::json!({"running": true})),
            )
            .unwrap();
        assert_eq!(
            owner.check(DEADLINE).unwrap(),
            serde_json::json!({"running": true})
        );
        telemetry.accepting.store(true, Ordering::Release);
        owner.stop();
        assert_eq!(
            drop_report.recv_timeout(DEADLINE).unwrap(),
            (next_thread, false)
        );
    }

    #[test]
    fn blocked_readback_times_out_disables_delivery_and_never_blocks_stop() {
        let mut owner = Owner::default();
        let telemetry = Arc::new(Telemetry::default());
        let worker_telemetry = Arc::clone(&telemetry);
        let (release, gate) = mpsc::sync_channel(1);
        let (entered, entry) = mpsc::sync_channel(1);
        let (dropped, drop_report) = mpsc::sync_channel(1);
        let owner_thread = owner
            .start_with(
                Arc::clone(&telemetry),
                DEADLINE,
                move |_| {
                    Ok((
                        Local::new(worker_telemetry, dropped),
                        thread::current().id(),
                    ))
                },
                move |_| {
                    entered.try_send(()).unwrap();
                    gate.recv_timeout(DEADLINE).unwrap();
                    Ok(Value::Null)
                },
            )
            .unwrap();
        telemetry.accepting.store(true, Ordering::Release);
        assert!(
            owner
                .check(BOUND)
                .unwrap_err()
                .contains("check did not complete")
        );
        entry.recv_timeout(DEADLINE).unwrap();
        assert!(!telemetry.accepting.load(Ordering::Acquire));
        let stopped = Instant::now();
        owner.stop();
        assert!(stopped.elapsed() < DEADLINE / 2);
        refused_retry(&mut owner);
        release.send(()).unwrap();
        assert_eq!(
            drop_report.recv_timeout(DEADLINE).unwrap(),
            (owner_thread, false)
        );
    }

    #[test]
    fn blocked_native_drop_is_retained_but_does_not_block_stop_or_stack_retries() {
        let mut owner = Owner::default();
        let telemetry = Arc::new(Telemetry::default());
        let worker_telemetry = Arc::clone(&telemetry);
        let (release, gate) = mpsc::sync_channel(1);
        let (dropping, drop_entered) = mpsc::sync_channel(1);
        let (dropped, drop_report) = mpsc::sync_channel(1);
        let owner_thread = owner
            .start_with(
                Arc::clone(&telemetry),
                DEADLINE,
                move |_| {
                    let mut local = Local::new(worker_telemetry, dropped);
                    local.drop_gate = Some(gate);
                    local.dropping = Some(dropping);
                    Ok((local, thread::current().id()))
                },
                |_| Ok(Value::Null),
            )
            .unwrap();
        telemetry.accepting.store(true, Ordering::Release);
        let stopped = Instant::now();
        owner.stop();
        assert!(stopped.elapsed() < DEADLINE / 2);
        assert!(!telemetry.accepting.load(Ordering::Acquire));
        drop_entered.recv_timeout(DEADLINE).unwrap();
        refused_retry(&mut owner);
        // Dropping the controller also never joins the blocked native owner.
        let stopped = Instant::now();
        drop(owner);
        assert!(stopped.elapsed() < DEADLINE / 2);
        release.send(()).unwrap();
        assert_eq!(
            drop_report.recv_timeout(DEADLINE).unwrap(),
            (owner_thread, false)
        );
    }
}
