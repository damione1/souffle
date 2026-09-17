//! Marshal AppKit-only work onto the real OS main thread from an arbitrary
//! background thread.
//!
//! Never call `on_main` (or `on_main_with(false, ..)`) from a context with no
//! pumped run loop (`cargo test`, headless CLI): `dispatch_sync` onto an
//! unpumped main queue hangs forever (SOU-122 AC3). Every current call site
//! is behind an early return on a `OnceLock` that only the real bootstrapped
//! GUI app ever populates, so tests never reach the hop.

#[cfg(target_os = "macos")]
pub fn on_main<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    on_main_with(is_main_thread(), f)
}

#[cfg(target_os = "macos")]
pub fn on_main_with<R: Send>(already_main: bool, f: impl FnOnce() -> R + Send) -> R {
    if already_main {
        return f();
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    dispatch2::DispatchQueue::main().exec_sync(move || {
        let _ = tx.send(f());
    });
    rx.recv().expect("main queue dropped a hop")
}

#[cfg(target_os = "macos")]
pub fn is_main_thread() -> bool {
    unsafe extern "C" {
        fn pthread_main_np() -> i32;
    }
    unsafe { pthread_main_np() != 0 }
}
