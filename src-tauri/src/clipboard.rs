use std::thread;
use std::time::Duration;

use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

use crate::settings::PasteMethod;

/// Insert text into the active application after `delay_ms`, using either
/// clipboard+Cmd+V or simulated keystrokes (for apps that reject synthetic paste).
pub fn paste_text(text: &str, delay_ms: u64, method: PasteMethod) -> Result<(), String> {
    thread::sleep(Duration::from_millis(delay_ms));

    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| format!("Enigo init: {e}"))?;

    match method {
        PasteMethod::Clipboard => {
            let mut clipboard =
                Clipboard::new().map_err(|e| format!("Clipboard init: {e}"))?;
            clipboard
                .set_text(text)
                .map_err(|e| format!("Clipboard set: {e}"))?;

            enigo
                .key(Key::Meta, Direction::Press)
                .map_err(|e| format!("Key press Meta: {e}"))?;
            enigo
                .key(Key::Unicode('v'), Direction::Click)
                .map_err(|e| format!("Key click V: {e}"))?;
            enigo
                .key(Key::Meta, Direction::Release)
                .map_err(|e| format!("Key release Meta: {e}"))?;
        }
        PasteMethod::Type => {
            enigo
                .text(text)
                .map_err(|e| format!("Simulated typing: {e}"))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_method_variants_exist() {
        assert_ne!(PasteMethod::Clipboard, PasteMethod::Type);
    }
}
