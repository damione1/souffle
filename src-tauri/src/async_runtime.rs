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

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new().expect("failed to create the tokio runtime"))
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
