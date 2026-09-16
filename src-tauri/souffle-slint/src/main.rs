// SOU-187: standalone Slint shell backed by a real headless Tauri App
// (souffle_lib::slint_bridge) - no webview, but the real AppState (audio
// thread, engine actor, database). AC5 pattern: state and actions cross the
// Rust<->UI boundary as plain Slint properties and callbacks - direct
// in-process function calls, no IPC, no serialization.
slint::include_modules!();

mod timeline;

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

/// Re-fetches dictations + meetings from the real database and rebuilds the
/// Timeline model - mirrors features/timeline/controller.svelte.ts's
/// `refresh()`. Reads the current filter/search straight off the window
/// (already the source of truth via its in-out properties) rather than
/// threading them through as parameters.
fn refresh_timeline(window: &MainWindow, tauri_handle: &AppHandle) {
    let kind_filter = window.get_kind_filter().to_string();
    let search_query = window.get_search_query().to_string();

    let state = tauri_handle.state::<souffle_lib::state::AppState>();
    let dictations = match souffle_lib::commands::list_dictation_entries(state, Some(200)) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("Failed to list dictation entries: {e}");
            Vec::new()
        }
    };
    let state = tauri_handle.state::<souffle_lib::state::AppState>();
    let meetings = match souffle_lib::commands::list_meetings(state) {
        Ok(meetings) => meetings,
        Err(e) => {
            eprintln!("Failed to list meetings: {e}");
            Vec::new()
        }
    };

    let is_empty = dictations.is_empty() && meetings.is_empty();
    let groups = timeline::build_groups(&dictations, &meetings, &kind_filter, &search_query);
    window.set_timeline_has_matches(!groups.is_empty());
    window.set_timeline_groups(std::rc::Rc::new(slint::VecModel::from(groups)).into());
    window.set_timeline_is_empty(is_empty);
}

/// Milestone 3 wires the real Timeline (this function); milestones 4-6 wire
/// dictate/meeting start and opening a meeting's detail. Until then those
/// two callbacks only log - no fabricated state change on click.
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
    let handle = tauri_handle.clone();
    window.on_filter_changed(move |kind| {
        if let Some(window) = weak.upgrade() {
            window.set_kind_filter(kind);
            refresh_timeline(&window, &handle);
        }
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_search_changed(move |_query| {
        if let Some(window) = weak.upgrade() {
            refresh_timeline(&window, &handle);
        }
    });

    window.on_timeline_item_opened(|kind, id| {
        // MeetingDetail is milestone 6; dictation inline-expand is not
        // ported yet either - both real actions, neither wired yet.
        eprintln!(
            "timeline-item-opened: {kind} {id} (open flow not wired yet, see SOU-187 milestone 6)"
        );
    });

    let weak = window.as_weak();
    let handle = tauri_handle.clone();
    window.on_timeline_item_removed(move |kind, id| {
        let state = handle.state::<souffle_lib::state::AppState>();
        let result = if kind == "dictation" {
            souffle_lib::commands::delete_dictation_entry(state, id.to_string())
        } else {
            souffle_lib::commands::delete_meeting(state, id.to_string())
        };
        if let Err(e) = result {
            eprintln!("Failed to delete {kind} {id}: {e}");
        }
        if let Some(window) = weak.upgrade() {
            refresh_timeline(&window, &handle);
        }
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

    refresh_timeline(&window, &tauri_handle);
    wire_callbacks(&window, tauri_handle);

    window.run().expect("event loop failed");
}
