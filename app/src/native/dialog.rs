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
    // The panel appends the allowed extension itself. Seeding it with a name
    // that already ends in that extension works for types macOS knows
    // (md/json/vtt/ogg) but not for ones without a registered UTType: an
    // `.srt` export was saved as `name.srt.srt` (SOU-195-slice, seen live).
    // So seed the bare stem and let the panel add the extension once.
    panel.setNameFieldStringValue(&NSString::from_str(name_stem(file_name, extension)));
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

    Ok(Some(with_extension(
        PathBuf::from(path.to_string()),
        extension,
    )))
}

/// `file_name` without a trailing `.{extension}`, if it has one.
fn name_stem<'a>(file_name: &'a str, extension: &str) -> &'a str {
    file_name
        .strip_suffix(extension)
        .and_then(|rest| rest.strip_suffix('.'))
        .filter(|stem| !stem.is_empty())
        .unwrap_or(file_name)
}

/// `path` with `.{extension}` appended when the panel returned it without
/// one, so the saved file always carries the export's extension.
fn with_extension(mut path: PathBuf, extension: &str) -> PathBuf {
    let has_it = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(extension));
    if !has_it {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(".");
        name.push(extension);
        path.set_file_name(name);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_stem_drops_only_the_matching_extension() {
        assert_eq!(name_stem("2026-09-23-sync.srt", "srt"), "2026-09-23-sync");
        assert_eq!(name_stem("2026-09-23-sync.md", "srt"), "2026-09-23-sync.md");
        assert_eq!(name_stem("notes-srt", "srt"), "notes-srt");
        assert_eq!(name_stem(".srt", "srt"), ".srt");
    }

    #[test]
    fn with_extension_appends_once() {
        assert_eq!(
            with_extension(PathBuf::from("/tmp/a.srt"), "srt"),
            PathBuf::from("/tmp/a.srt")
        );
        assert_eq!(
            with_extension(PathBuf::from("/tmp/a"), "srt"),
            PathBuf::from("/tmp/a.srt")
        );
        assert_eq!(
            with_extension(PathBuf::from("/tmp/a.v1"), "vtt"),
            PathBuf::from("/tmp/a.v1.vtt")
        );
    }
}
