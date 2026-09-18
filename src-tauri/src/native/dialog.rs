//! Native replacement for `tauri-plugin-dialog` (SOU-191).
//!
//! `NSSavePanel` via `objc2-app-kit`, already a dependency (the tray/pill use
//! it). The Tauri-era version sheeted the panel onto the `main` webview
//! window (`set_parent`); Slint's window is owned by the `souffle-slint`
//! crate and not reachable from here, so this shows a plain app-modal panel
//! instead of a window-attached sheet — a minor visual difference (no slide-
//! down animation off a specific window), not a functional one. Documented
//! here rather than silently changed.
//!
//! Must be called from the thread with the live run loop (Slint's main
//! thread) — same requirement `dictation_cancel.rs` already documented for
//! `CGEventTap`/shortcut registration. Callers in `souffle-slint` already
//! have a `run_on_main_thread` helper for exactly this.

use std::path::PathBuf;

use objc2::MainThreadMarker;
use objc2_app_kit::{NSModalResponseOK, NSSavePanel};
use objc2_foundation::NSString;

/// Show a native save panel. Returns `Ok(None)` if the user cancels.
pub fn pick_save_path(file_name: &str, extension: &str) -> Result<Option<PathBuf>, String> {
    crate::tray::activate_app();

    let Some(mtm) = MainThreadMarker::new() else {
        return Err("Save dialog must run on the main thread".into());
    };

    let panel = NSSavePanel::savePanel(mtm);
    panel.setNameFieldStringValue(&NSString::from_str(file_name));
    // `allowedContentTypes` needs a `UTType` lookup; the deprecated
    // extension-list API is still the simplest correct way to filter a save
    // panel by a bare extension string (mirrors `tray::activate_app`'s own
    // `#[allow(deprecated)]` for the same "still the only API that works"
    // reason).
    #[allow(deprecated)]
    panel.setAllowedFileTypes(Some(&objc2_foundation::NSArray::from_slice(&[
        NSString::from_str(extension).as_ref(),
    ])));

    let response = panel.runModal();
    if response != NSModalResponseOK {
        return Ok(None);
    }

    let Some(url) = panel.URL() else {
        return Ok(None);
    };
    let Some(path) = url.path() else {
        return Err("Save panel returned a URL with no file path".into());
    };

    Ok(Some(PathBuf::from(path.to_string())))
}
