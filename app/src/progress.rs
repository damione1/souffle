//! Replacement for `tauri::ipc::Channel<T>` (SOU-191).
//!
//! The Tauri channel existed to cross the webview IPC boundary: a command
//! serializes `T` to JSON and a JS listener on the other side deserializes
//! it. Slint has no such boundary — the UI and the command body are the same
//! process, same address space — so this is just a plain callback with the
//! same call-site shape (`Channel::new(...)`, `channel.send(value)`), kept so
//! the command bodies that already used it did not need restructuring.

use std::sync::Arc;

/// A cloneable, thread-safe sink for progress values of type `T`.
pub struct ProgressChannel<T>(Arc<dyn Fn(T) + Send + Sync>);

impl<T> ProgressChannel<T> {
    pub fn new(f: impl Fn(T) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    pub fn send(&self, value: T) {
        (self.0)(value);
    }
}

impl<T> Clone for ProgressChannel<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> std::fmt::Debug for ProgressChannel<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProgressChannel(..)")
    }
}
