use std::thread;
use std::time::Duration;

use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

/// Copy text to clipboard and simulate Cmd+V to paste into the active application.
/// `delay_ms` controls the pause between clipboard set and keystroke simulation.
pub fn copy_and_paste(text: &str, delay_ms: u64) -> Result<(), String> {
    // Set clipboard
    let mut clipboard = Clipboard::new().map_err(|e| format!("Clipboard init: {e}"))?;
    clipboard
        .set_text(text)
        .map_err(|e| format!("Clipboard set: {e}"))?;

    // Wait for the app to regain focus after our window loses it
    thread::sleep(Duration::from_millis(delay_ms));

    // Simulate Cmd+V
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| format!("Enigo init: {e}"))?;
    enigo
        .key(Key::Meta, Direction::Press)
        .map_err(|e| format!("Key press Meta: {e}"))?;
    enigo
        .key(Key::Unicode('v'), Direction::Click)
        .map_err(|e| format!("Key click V: {e}"))?;
    enigo
        .key(Key::Meta, Direction::Release)
        .map_err(|e| format!("Key release Meta: {e}"))?;

    Ok(())
}
