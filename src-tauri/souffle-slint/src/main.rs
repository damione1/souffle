// SOU-187: standalone Slint shell backed by a real headless Tauri App
// (souffle_lib::slint_bridge) - no webview, but the real AppState (audio
// thread, engine actor, database). AC5 pattern: state and actions cross the
// Rust<->UI boundary as plain Slint properties and callbacks - direct
// in-process function calls, no IPC, no serialization.
slint::include_modules!();

use tauri::Manager;

fn main() {
    // Real bootstrap (audio thread, engine actor, DB, replayed .setup()) -
    // see slint_bridge.rs. Must run before Slint's own window/event loop.
    let tauri_app = souffle_lib::slint_bridge::build();
    let tauri_handle = tauri_app.handle().clone();
    // Never call .run() on this App; it must simply stay alive so the
    // AppHandle above keeps working while Slint owns the OS event loop.
    std::mem::forget(tauri_app);

    let window = MainWindow::new().expect("failed to create Slint window");

    let weak = window.as_weak();
    window.on_ping_requested(move || {
        // Real backend command, real database - not a stand-in counter.
        let state = tauri_handle.state::<souffle_lib::state::AppState>();
        let status = match souffle_lib::commands::list_meetings(state) {
            Ok(meetings) => format!("{} meeting(s) in database", meetings.len()),
            Err(e) => format!("list_meetings failed: {e}"),
        };
        if let Some(window) = weak.upgrade() {
            window.set_status_text(status.into());
        }
    });

    window.run().expect("event loop failed");
}
