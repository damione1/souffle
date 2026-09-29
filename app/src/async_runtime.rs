//! Global tokio runtime (SOU-191).
//!
//! `tauri::async_runtime::{spawn, spawn_blocking, block_on}` were thin
//! wrappers over a runtime Tauri created and stashed in a global static, so
//! every call site — regardless of which thread it ran on — could reach it.
//! This replaces that global with the same shape, backed by a plain
//! `tokio::runtime::Runtime`. Deliberately *not* `tokio::task::spawn` (the
//! free function): that one requires the calling thread to already be
//! "inside" a runtime (an `EnterGuard` or a worker thread), which plain
//! `std::thread::spawn` threads (the audio thread, engine actor, tray/global
//! -hotkey event loops, …) never are. `Runtime::spawn`/`spawn_blocking` are
//! plain methods that work from any thread.
//!
//! SOU-289: sized for what actually runs on it. `Runtime::new()` started one
//! worker per core (12 on the reference machine) for two background tasks
//! (calendar, update check); everything heavy goes through `spawn_blocking`.
//! Two workers are enough, and a 90 s keep-alive stops the calendar's 60 s
//! tick from creating and tearing down a blocking thread every minute.
//!
//! [`block_on`] must never be called from one of this runtime's own threads
//! (a worker or a `spawn_blocking` closure): tokio panics on a nested
//! `block_on`, and with only two workers a blocked one would starve the rest.
//! Today's callers all satisfy this: the Slint UI thread
//! (souffle-slint `main.rs`) and the dedicated stop thread in
//! `commands/transcription.rs`.

use std::future::Future;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

const WORKER_THREADS: usize = 2;
const MAX_BLOCKING_THREADS: usize = 8;
const BLOCKING_KEEP_ALIVE: Duration = Duration::from_secs(90);

fn build_runtime() -> std::io::Result<Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(WORKER_THREADS)
        .max_blocking_threads(MAX_BLOCKING_THREADS)
        .thread_keep_alive(BLOCKING_KEEP_ALIVE)
        .thread_name("tokio-rt-worker")
        .enable_all()
        .build()
}

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| build_runtime().expect("failed to create the tokio runtime"))
}

pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    runtime().spawn(future)
}

pub fn spawn_blocking<F, R>(f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    runtime().spawn_blocking(f)
}

pub fn block_on<F: Future>(future: F) -> F::Output {
    runtime().block_on(future)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_has_two_named_workers() {
        let rt = build_runtime().expect("runtime builds");
        assert_eq!(rt.metrics().num_workers(), WORKER_THREADS);
        let name = rt.block_on(async {
            tokio::spawn(async { std::thread::current().name().map(str::to_owned) })
                .await
                .expect("task joins")
        });
        assert_eq!(name.as_deref(), Some("tokio-rt-worker"));
    }

    #[test]
    fn timers_and_blocking_pool_work_on_the_trimmed_runtime() {
        let rt = build_runtime().expect("runtime builds");
        let out = rt.block_on(async {
            tokio::time::sleep(Duration::from_millis(1)).await;
            tokio::task::spawn_blocking(|| 21 * 2)
                .await
                .expect("blocking task joins")
        });
        assert_eq!(out, 42);
    }
}
