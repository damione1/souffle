//! Turns the raw `KeyEvent` fields a Slint `FocusScope` hands over (a
//! produced `text` character plus modifier booleans) into the same
//! accelerator strings the backend already stores (`CommandOrControl+Shift+D`,
//! `MetaLeft`) - mirrors `src/lib/utils/shortcut.ts`'s
//! `keyEventToShortcut`/`modifierToShortcut`/`shortcutMissingModifier`.
//!
//! Slint gives a produced character (or one of `i-slint-common::key_codes`'s
//! private-use code points for named/modifier keys), not a DOM physical
//! `code`, so this matches against those code points directly instead of
//! going through Slint's `.slint`-only `Key` namespace. One known gap versus
//! the Svelte version: non-QWERTY layouts and `AltGr` are not distinguished
//! from a plain right Option key the way a browser's `code` would.

/// The modifiers held alongside a key press, as Slint's `KeyEvent.modifiers`
/// reports them.
#[derive(Debug, Clone, Copy, Default)]
pub struct Modifiers {
    pub control: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Modifiers {
    fn any(self) -> bool {
        self.control || self.shift || self.alt || self.meta
    }
}

/// A bare modifier key press on its own (mirrors `modifierToShortcut()`).
pub fn modifier_only_shortcut(text: &str) -> Option<&'static str> {
    Some(match text {
        "\u{10}" => "ShiftLeft",
        "\u{15}" => "ShiftRight",
        "\u{11}" => "ControlLeft",
        "\u{16}" => "ControlRight",
        "\u{12}" => "AltLeft",
        // AltGr: the right Option key on macOS.
        "\u{13}" => "AltRight",
        "\u{17}" => "MetaLeft",
        "\u{18}" => "MetaRight",
        _ => return None,
    })
}

fn named_key(text: &str) -> Option<&'static str> {
    Some(match text {
        "\u{20}" => "Space",
        "\u{a}" => "Enter",
        "\u{1b}" => "Escape",
        "\u{8}" => "Backspace",
        "\u{9}" => "Tab",
        "\u{7f}" => "Delete",
        "\u{f700}" => "ArrowUp",
        "\u{f701}" => "ArrowDown",
        "\u{f702}" => "ArrowLeft",
        "\u{f703}" => "ArrowRight",
        "\u{f729}" => "Home",
        "\u{f72b}" => "End",
        "\u{f72c}" => "PageUp",
        "\u{f72d}" => "PageDown",
        "\u{f704}" => "F1",
        "\u{f705}" => "F2",
        "\u{f706}" => "F3",
        "\u{f707}" => "F4",
        "\u{f708}" => "F5",
        "\u{f709}" => "F6",
        "\u{f70a}" => "F7",
        "\u{f70b}" => "F8",
        "\u{f70c}" => "F9",
        "\u{f70d}" => "F10",
        "\u{f70e}" => "F11",
        "\u{f70f}" => "F12",
        _ => return None,
    })
}

/// Maps a produced character back to the physical key name the backend
/// expects (mirrors `mapKey()`, which reads the DOM `code`; Slint only hands
/// over the produced character, so a shifted symbol is normalized back to
/// its unshifted physical key here).
fn punctuation_key(text: &str) -> Option<&'static str> {
    Some(match text {
        "`" | "~" => "Backquote",
        "-" | "_" => "Minus",
        "=" | "+" => "Equal",
        "[" | "{" => "BracketLeft",
        "]" | "}" => "BracketRight",
        "\\" | "|" => "Backslash",
        ";" | ":" => "Semicolon",
        "'" | "\"" => "Quote",
        "," | "<" => "Comma",
        "." | ">" => "Period",
        "/" | "?" => "Slash",
        "0" | ")" => "0",
        "1" | "!" => "1",
        "2" | "@" => "2",
        "3" | "#" => "3",
        "4" | "$" => "4",
        "5" | "%" => "5",
        "6" | "^" => "6",
        "7" | "&" => "7",
        "8" | "*" => "8",
        "9" | "(" => "9",
        _ => return None,
    })
}

/// True for a bare letter/digit/symbol key with no modifier and no function
/// key held (mirrors `shortcutMissingModifier()`).
pub fn missing_modifier(text: &str, modifiers: Modifiers) -> bool {
    if modifiers.any() {
        return false;
    }
    !matches!(named_key(text), Some(key) if key.starts_with('F'))
}

/// Builds the accelerator string the backend stores from a non-modifier key
/// press plus the modifiers held at the same time (mirrors
/// `keyEventToShortcut()`). Returns `None` for a bare modifier press or an
/// unmapped key.
pub fn format_combo(text: &str, modifiers: Modifiers) -> Option<String> {
    if modifier_only_shortcut(text).is_some() {
        return None;
    }
    let key = named_key(text)
        .or_else(|| punctuation_key(text))
        .map(str::to_string)
        .or_else(|| {
            let ch = text.chars().next()?;
            ch.is_ascii_alphabetic()
                .then(|| ch.to_ascii_uppercase().to_string())
        })?;

    let mut parts = Vec::new();
    if modifiers.control || modifiers.meta {
        parts.push("CommandOrControl");
    }
    if modifiers.shift {
        parts.push("Shift");
    }
    if modifiers.alt {
        parts.push("Alt");
    }
    parts.push(&key);
    Some(parts.join("+"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_letter_without_modifier_has_no_combo() {
        assert_eq!(
            format_combo("d", Modifiers::default()),
            Some("D".to_string())
        );
    }

    #[test]
    fn missing_modifier_flags_bare_letter() {
        assert!(missing_modifier("d", Modifiers::default()));
    }

    #[test]
    fn function_key_does_not_need_a_modifier() {
        assert!(!missing_modifier("\u{f708}", Modifiers::default()));
        assert_eq!(
            format_combo("\u{f708}", Modifiers::default()),
            Some("F5".to_string())
        );
    }

    #[test]
    fn combo_with_command_and_shift() {
        let modifiers = Modifiers {
            meta: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            format_combo("d", modifiers),
            Some("CommandOrControl+Shift+D".to_string())
        );
    }

    #[test]
    fn shifted_punctuation_normalizes_to_the_physical_key() {
        let modifiers = Modifiers {
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            format_combo("_", modifiers),
            Some("Shift+Minus".to_string())
        );
    }

    #[test]
    fn bare_modifier_press_has_no_combo() {
        assert_eq!(format_combo("\u{17}", Modifiers::default()), None);
        assert_eq!(modifier_only_shortcut("\u{17}"), Some("MetaLeft"));
    }

    #[test]
    fn unmapped_key_has_no_combo() {
        assert_eq!(format_combo("\u{3b1}", Modifiers::default()), None);
    }
}
