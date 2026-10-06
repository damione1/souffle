//! What a physical key is labelled on the user's keyboard layout (#360).
//!
//! Stored accelerators name keys by their US ANSI position (`Q`, `Minus`),
//! because that is what `global-hotkey` registers. On AZERTY the key stored
//! as `Q` is the one labelled A, so the settings label asks the current
//! layout what that key types, the way KeyboardShortcuts (`Shortcut.swift`)
//! and MASShortcut (`MASShortcut.m`) display shortcuts:
//! `TISCopyCurrentASCIICapableKeyboardLayoutInputSource` →
//! `kTISPropertyUnicodeKeyLayoutData` → `UCKeyTranslate` in display mode,
//! no modifiers, no dead keys. The ASCII-capable source is what AppKit menus
//! use too, so a Russian or Greek layout shows the Latin letter of its
//! companion layout instead of Cyrillic.
//!
//! Text Input Sources are main-thread-only (TSM asserts the main queue on
//! recent macOS), so the lookup takes a `MainThreadMarker`: it cannot be
//! called from the shortcut tap thread or a Tokio worker.

use std::ffi::c_void;

use objc2::MainThreadMarker;

/// `kUCKeyActionDisplay` (UnicodeUtilities.h): the key's label, not typing.
const UC_KEY_ACTION_DISPLAY: u16 = 3;
/// `kUCKeyTranslateNoDeadKeysMask`: a dead key (`^` on AZERTY) shows its
/// own mark instead of waiting for the next key.
const UC_KEY_TRANSLATE_NO_DEAD_KEYS_MASK: u32 = 1;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    /// Returns a +1 retained `TISInputSourceRef`, or null.
    fn TISCopyCurrentASCIICapableKeyboardLayoutInputSource() -> *mut c_void;
    /// Get rule: the returned `CFDataRef` is owned by `source`.
    fn TISGetInputSourceProperty(source: *mut c_void, key: *const c_void) -> *mut c_void;
    static kTISPropertyUnicodeKeyLayoutData: *const c_void;
    fn LMGetKbdType() -> u8;
    fn UCKeyTranslate(
        layout: *const c_void,
        virtual_key_code: u16,
        key_action: u16,
        modifier_key_state: u32,
        keyboard_type: u32,
        key_translate_options: u32,
        dead_key_state: *mut u32,
        max_string_length: usize,
        actual_string_length: *mut usize,
        unicode_string: *mut u16,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    fn CFRelease(cf: *const c_void);
}

/// macOS virtual keycode (HIToolbox `Events.h`) of an accelerator key token
/// whose label depends on the layout: letters, digits, punctuation. Named
/// keys (Space, arrows, F-keys) and modifiers keep their own label, so they
/// have none.
pub fn virtual_key_code(token: &str) -> Option<u16> {
    Some(match token {
        "A" => 0x00,
        "S" => 0x01,
        "D" => 0x02,
        "F" => 0x03,
        "H" => 0x04,
        "G" => 0x05,
        "Z" => 0x06,
        "X" => 0x07,
        "C" => 0x08,
        "V" => 0x09,
        "B" => 0x0b,
        "Q" => 0x0c,
        "W" => 0x0d,
        "E" => 0x0e,
        "R" => 0x0f,
        "Y" => 0x10,
        "T" => 0x11,
        "1" => 0x12,
        "2" => 0x13,
        "3" => 0x14,
        "4" => 0x15,
        "6" => 0x16,
        "5" => 0x17,
        "Equal" => 0x18,
        "9" => 0x19,
        "7" => 0x1a,
        "Minus" => 0x1b,
        "8" => 0x1c,
        "0" => 0x1d,
        "BracketRight" => 0x1e,
        "O" => 0x1f,
        "U" => 0x20,
        "BracketLeft" => 0x21,
        "I" => 0x22,
        "P" => 0x23,
        "L" => 0x25,
        "J" => 0x26,
        "Quote" => 0x27,
        "K" => 0x28,
        "Semicolon" => 0x29,
        "Backslash" => 0x2a,
        "Comma" => 0x2b,
        "Slash" => 0x2c,
        "N" => 0x2d,
        "M" => 0x2e,
        "Period" => 0x2f,
        "Backquote" => 0x32,
        _ => return None,
    })
}

/// Turns what `UCKeyTranslate` produced into a label: uppercased like a
/// keycap, except where uppercasing changes the length (`ß` stays `ß`, not
/// `SS`). Nothing printable (a control character, a blank) means the layout
/// has no useful label for the key.
pub fn display_text(produced: &str) -> Option<String> {
    if produced.is_empty()
        || produced
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
    {
        return None;
    }
    Some(
        produced
            .chars()
            .map(|c| {
                let mut upper = c.to_uppercase();
                match (upper.next(), upper.next()) {
                    (Some(single), None) => single,
                    _ => c,
                }
            })
            .collect(),
    )
}

/// The label of an accelerator key token on the current ASCII-capable
/// layout, or `None` when the token has no layout-dependent label or the
/// lookup fails (the caller keeps the US name).
pub fn key_label(mtm: MainThreadMarker, token: &str) -> Option<String> {
    let key_code = virtual_key_code(token)?;
    let produced = translate(mtm, key_code)?;
    display_text(&produced)
}

fn translate(_mtm: MainThreadMarker, key_code: u16) -> Option<String> {
    // SAFETY: main thread (the marker), no arguments. The result is either
    // null or a +1 reference released at the end of this function.
    let source = unsafe { TISCopyCurrentASCIICapableKeyboardLayoutInputSource() };
    if source.is_null() {
        return None;
    }
    let produced = (|| {
        // SAFETY: `source` is a live input source and the key is the
        // framework's own constant. The data follows the Get rule: owned by
        // `source`, which outlives this closure.
        let data = unsafe { TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData) };
        if data.is_null() {
            return None;
        }
        // SAFETY: `data` is a non-null CFDataRef holding a `UCKeyboardLayout`.
        let layout = unsafe { CFDataGetBytePtr(data) };
        if layout.is_null() {
            return None;
        }
        let mut dead_key_state = 0u32;
        let mut buffer = [0u16; 4];
        let mut length = 0usize;
        // SAFETY: `layout` points into `data`, still alive; every out
        // pointer is a local, and the buffer length passed is its real size.
        let status = unsafe {
            UCKeyTranslate(
                layout.cast(),
                key_code,
                UC_KEY_ACTION_DISPLAY,
                0,
                u32::from(LMGetKbdType()),
                UC_KEY_TRANSLATE_NO_DEAD_KEYS_MASK,
                &mut dead_key_state,
                buffer.len(),
                &mut length,
                buffer.as_mut_ptr(),
            )
        };
        if status != 0 {
            return None;
        }
        String::from_utf16(&buffer[..length.min(buffer.len())]).ok()
    })();
    // SAFETY: balances the Copy above; `source` is not used afterwards.
    unsafe { CFRelease(source) };
    produced
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_character_keys_have_a_layout_label() {
        assert_eq!(virtual_key_code("A"), Some(0x00));
        assert_eq!(virtual_key_code("Q"), Some(0x0c));
        assert_eq!(virtual_key_code("1"), Some(0x12));
        assert_eq!(virtual_key_code("Backquote"), Some(0x32));
        assert_eq!(virtual_key_code("Minus"), Some(0x1b));
        for named in [
            "Space", "Enter", "ArrowUp", "F5", "Numpad1", "Shift", "MetaLeft",
        ] {
            assert_eq!(virtual_key_code(named), None, "{named}");
        }
    }

    /// Every letter, digit and punctuation name the recorder stores has a
    /// keycode, and no two share one.
    #[test]
    fn character_keycodes_are_distinct() {
        let tokens: Vec<String> = ('A'..='Z')
            .chain('0'..='9')
            .map(String::from)
            .chain(
                [
                    "Backquote",
                    "Minus",
                    "Equal",
                    "BracketLeft",
                    "BracketRight",
                    "Backslash",
                    "Semicolon",
                    "Quote",
                    "Comma",
                    "Period",
                    "Slash",
                ]
                .map(String::from),
            )
            .collect();
        let mut codes: Vec<u16> = tokens
            .iter()
            .map(|t| virtual_key_code(t).unwrap_or_else(|| panic!("{t}")))
            .collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), tokens.len());
    }

    #[test]
    fn labels_are_uppercased_like_a_keycap() {
        assert_eq!(display_text("a").as_deref(), Some("A"));
        assert_eq!(display_text("é").as_deref(), Some("É"));
        assert_eq!(display_text("&").as_deref(), Some("&"));
        assert_eq!(display_text("ß").as_deref(), Some("ß"));
    }

    #[test]
    fn unprintable_output_has_no_label() {
        assert_eq!(display_text(""), None);
        assert_eq!(display_text(" "), None);
        assert_eq!(display_text("\u{10}"), None);
    }
}
