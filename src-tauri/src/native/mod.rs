//! Native macOS replacements for the Tauri plugins removed in SOU-191.
//!
//! Each submodule replaces exactly one plugin (see the ticket's scope table):
//! `shortcuts` (`tauri-plugin-global-shortcut`), `notifications`
//! (`tauri-plugin-notification`), `dialog` (`tauri-plugin-dialog`),
//! `single_instance` (`tauri-plugin-single-instance`), `tray` (the
//! `tray-icon` feature of `tauri`), `updater` (`tauri-plugin-updater`).
//! `tauri-plugin-fs` and `tauri-plugin-log` had no Rust-side caller (grepped,
//! not assumed) and are dropped with no replacement.

pub mod bridge;
pub mod dialog;
pub mod notifications;
pub mod shortcuts;
pub mod single_instance;
pub mod updater;
