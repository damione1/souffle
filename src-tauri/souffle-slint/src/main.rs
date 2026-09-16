// SOU-186: standalone Slint shell, no Tauri anywhere in this crate's dependency graph.
//
// AC5 pattern: state and actions cross the Rust<->UI boundary as plain Slint
// properties and callbacks - direct in-process function calls, no JSON, no
// specta-generated bindings, no serialization at all. `ping_count` below
// stands in for real backend state (today: AppState behind specta commands);
// `on_ping_requested` stands in for a Tauri command handler.
slint::include_modules!();

use std::cell::Cell;
use std::rc::Rc;

fn main() {
    let window = MainWindow::new().expect("failed to create Slint window");

    // Rust-owned state, not UI state - the pattern any real backend
    // (AppState, pipeline status, settings) follows.
    let ping_count = Rc::new(Cell::new(0u32));

    let weak = window.as_weak();
    window.on_ping_requested(move || {
        ping_count.set(ping_count.get() + 1);
        // Push new state into the UI: a direct setter call, not a message.
        if let Some(window) = weak.upgrade() {
            window.set_status_text(format!("pinged {} time(s)", ping_count.get()).into());
        }
    });

    window.run().expect("event loop failed");
}
