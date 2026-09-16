//! Native macOS tray item (NSStatusItem) replacement for Tauri tray plugin.

#[cfg(target_os = "macos")]
use objc2_app_kit::{NSMenu, NSMenuItem, NSStatusBar, NSVariableStatusItemLength};
#[cfg(target_os = "macos")]
use objc2_foundation::{MainThreadMarker, NSString};

pub struct NativeTray;

impl NativeTray {
    pub fn init() {
        #[cfg(target_os = "macos")]
        if let Some(mtm) = MainThreadMarker::new() {
            unsafe {
                let status_bar = NSStatusBar::systemStatusBar();
                let status_item = status_bar.statusItemWithLength(NSVariableStatusItemLength);

                if let Some(button) = status_item.button(mtm) {
                    let title = NSString::from_str("Soufflé");
                    button.setTitle(&title);
                }

                let menu = NSMenu::new(mtm);
                let quit_title = NSString::from_str("Quitter Soufflé");
                let key = NSString::from_str("q");
                let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                    mtm.alloc(),
                    &quit_title,
                    None,
                    &key,
                );
                menu.addItem(&item);
                status_item.setMenu(Some(&menu));
            }
        }
    }
}
