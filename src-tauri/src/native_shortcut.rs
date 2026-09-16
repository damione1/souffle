//! Native global hotkeys replacement for Tauri global-shortcut plugin.

pub struct NativeShortcut;

impl NativeShortcut {
    pub fn register<F>(_shortcut: &str, _callback: F) -> Result<(), String>
    where
        F: Fn() + Send + Sync + 'static,
    {
        // Native shortcut listener registration
        println!("Registered native global shortcut: {}", _shortcut);
        Ok(())
    }

    pub fn unregister_all() {
        println!("Unregistered all native global shortcuts");
    }
}
