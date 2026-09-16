//! Native NSOpenPanel / NSSavePanel replacement for Tauri dialog plugin.

#[cfg(target_os = "macos")]
use objc2_app_kit::{NSModalResponseOK, NSOpenPanel, NSSavePanel};
#[cfg(target_os = "macos")]
use objc2_foundation::MainThreadMarker;
use std::path::PathBuf;

pub struct NativeDialog;

impl NativeDialog {
    pub fn open_file(title: &str) -> Option<PathBuf> {
        #[cfg(target_os = "macos")]
        if let Some(mtm) = MainThreadMarker::new() {
            let panel = NSOpenPanel::openPanel(mtm);
            panel.setCanChooseFiles(true);
            panel.setCanChooseDirectories(false);
            panel.setAllowsMultipleSelection(false);

            if panel.runModal() == NSModalResponseOK {
                let urls = panel.URLs();
                if urls.count() > 0 {
                    let url = urls.objectAtIndex(0);
                    if let Some(path_ns) = url.path() {
                        return Some(PathBuf::from(path_ns.to_string()));
                    }
                }
            }
        }
        let _ = title;
        None
    }

    pub fn save_file(title: &str, default_filename: &str) -> Option<PathBuf> {
        #[cfg(target_os = "macos")]
        if let Some(mtm) = MainThreadMarker::new() {
            use objc2_foundation::NSString;
            let panel = NSSavePanel::savePanel(mtm);
            let name_ns = NSString::from_str(default_filename);
            panel.setNameFieldStringValue(&name_ns);

            if panel.runModal() == NSModalResponseOK {
                let url_opt = panel.URL();
                let path_opt = url_opt.as_ref().and_then(|u| u.path());
                if let Some(path_ns) = path_opt {
                    return Some(PathBuf::from(path_ns.to_string()));
                }
            }
        }
        let _ = (title, default_filename);
        None
    }
}
