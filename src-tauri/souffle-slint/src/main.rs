// SOU-187: standalone Slint shell backed by a real headless Tauri App
// (souffle_lib::slint_bridge) - no webview, but the real AppState (audio
// thread, engine actor, database). AC5 pattern: state and actions cross the
// Rust<->UI boundary as plain Slint properties and callbacks - direct
// in-process function calls, no IPC, no serialization.
slint::include_modules!();

use tauri::{AppHandle, Manager};

// Port of src/lib/utils/format.ts::formatShortcutLabel.
fn format_shortcut_label(shortcut: &str) -> String {
    if shortcut.is_empty() {
        return String::new();
    }
    shortcut
        .replace("CommandOrControl", "\u{2318}")
        .replace("Shift", "\u{21e7}")
        .replace("Alt", "\u{2325}")
        .replace('+', " ")
}

/// Milestone 2: shell + real state on load (shortcuts, whether the DB has
/// any history). Milestone 3 wires the actual Timeline list; milestones 4-5
/// wire dictate/meeting start. Until then these callbacks only log - no
/// fabricated state change on click.
///
/// Plain `eprintln!`, not `tracing`: this dev shell's own diagnostics are
/// unrelated to the production log file/filter (`SOUFFLE_LOG`, scoped to the
/// `souffle` lib crate's own targets), which was the wrong tool here and
/// silently swallowed these lines during milestone 2 verification.
fn wire_callbacks(window: &MainWindow, tauri_handle: AppHandle) {
    window.on_dictate_requested(|| {
        eprintln!("dictate-requested (start flow not wired yet, see SOU-187 milestone 4)");
    });
    window.on_meeting_requested(|| {
        eprintln!("meeting-requested (start flow not wired yet, see SOU-187 milestone 4)");
    });
    let weak = window.as_weak();
    window.on_filter_changed(move |kind| {
        eprintln!("filter-changed: {kind} (Timeline not wired yet, see SOU-187 milestone 3)");
        if let Some(window) = weak.upgrade() {
            window.set_kind_filter(kind);
        }
    });
    window.on_search_changed(|query| {
        eprintln!("search-changed: {query} (Timeline not wired yet, see SOU-187 milestone 3)");
    });

    let weak = window.as_weak();
    window.on_dismiss_transcription_status(move || {
        if let Some(window) = weak.upgrade() {
            window.set_transcription_status_message("".into());
        }
    });
    let weak = window.as_weak();
    window.on_dismiss_meeting_status(move || {
        if let Some(window) = weak.upgrade() {
            window.set_meeting_status_message("".into());
        }
    });

    let _ = tauri_handle;
}

fn main() {
    // Real bootstrap (audio thread, engine actor, DB, replayed .setup()) -
    // see slint_bridge.rs. Must run before Slint's own window/event loop.
    let tauri_app = souffle_lib::slint_bridge::build();
    let tauri_handle = tauri_app.handle().clone();
    // Never call .run() on this App; it must simply stay alive so the
    // AppHandle above keeps working while Slint owns the OS event loop.
    std::mem::forget(tauri_app);

    let window = MainWindow::new().expect("failed to create Slint window");

    // Real shortcut setting, not a placeholder - mirrors HomeView.svelte's
    // onMount getShortcuts() call.
    let state = tauri_handle.state::<souffle_lib::state::AppState>();
    match souffle_lib::commands::get_shortcuts(state) {
        Ok(shortcuts) => {
            window.set_dictation_shortcut(format_shortcut_label(&shortcuts.toggle).into());
        }
        Err(e) => eprintln!("Failed to load shortcuts: {e}"),
    }

    // Real DB query, not a hardcoded true - mirrors HomeView.svelte's
    // onMount timeline.refresh() as far as milestone 2 goes (empty state
    // only; the actual list is milestone 3).
    let state = tauri_handle.state::<souffle_lib::state::AppState>();
    match souffle_lib::commands::list_meetings(state) {
        Ok(meetings) => window.set_timeline_is_empty(meetings.is_empty()),
        Err(e) => eprintln!("Failed to list meetings: {e}"),
    }

    wire_callbacks(&window, tauri_handle);

    window.run().expect("event loop failed");
}
