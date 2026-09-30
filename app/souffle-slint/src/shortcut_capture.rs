//! Turns a key press captured by the shortcut recorder into the accelerator
//! strings the backend stores (`CommandOrControl+Shift+D`, `MetaLeft`) —
//! mirrors `src/lib/utils/shortcut.ts`'s
//! `keyEventToShortcut`/`modifierToShortcut`/`shortcutMissingModifier`.
//!
//! The key itself comes from winit's `physical_key`, not from Slint's
//! `KeyEvent.text` (#360). Slint only hands over the produced character, and
//! both consumers of the stored string work on physical keys: `global-hotkey`
//! registers a Carbon hotkey on the virtual keycode of the ANSI position its
//! key name denotes, and the native tap matches `MetaLeft`/`AltRight`/… by
//! keycode. Mapping the produced character back to a key name only worked on
//! US QWERTY: on AZERTY `⌘A` was stored as `A` and fired on the key labelled
//! Q, `&` became `7`, a Cyrillic or Greek letter mapped to nothing, and the
//! left and right Option keys were indistinguishable (winit never reports a
//! location for Option on macOS). The physical key is what the user pressed on
//! every layout, left and right included, and needs no extra permission:
//! winit reads it from the `NSEvent` the window already receives.
//!
//! `observe_key_event` is fed from the single winit window-event hook
//! (`wire_window_activity` in `main.rs`), which runs synchronously right
//! before Slint dispatches the same event to the recorder's `FocusScope`, so
//! `take_physical_key` in the capture callback reads the key of that event.

use std::cell::Cell;

use slint::winit_030::winit::event::KeyEvent;
use slint::winit_030::winit::keyboard::{KeyCode, PhysicalKey};

thread_local! {
    /// Physical key of the last winit keyboard event, consumed by the capture
    /// callback that event triggers. UI thread only.
    static LAST_PHYSICAL_KEY: Cell<Option<KeyCode>> = const { Cell::new(None) };
}

/// Records the physical key of a winit keyboard event (press or release).
pub fn observe_key_event(event: &KeyEvent) {
    let key = match event.physical_key {
        PhysicalKey::Code(code) => Some(code),
        PhysicalKey::Unidentified(_) => None,
    };
    LAST_PHYSICAL_KEY.with(|slot| slot.set(key));
}

/// The physical key of the event being dispatched, taken so a later Slint
/// event that did not come through winit cannot reuse it.
pub fn take_physical_key() -> Option<KeyCode> {
    LAST_PHYSICAL_KEY.with(Cell::take)
}

/// The modifiers held alongside a key press, named after the physical keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub command: bool,
    pub control: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Modifiers {
    /// Converts Slint's `KeyEvent.modifiers`. On Apple platforms Slint maps
    /// the Command key to `control` and the Control key to `meta`, like Qt
    /// (`i-slint-backend-winit`'s `swap_cmd_ctrl`), so they are swapped back
    /// here.
    pub fn from_slint(control: bool, shift: bool, alt: bool, meta: bool) -> Self {
        Self {
            command: control,
            control: meta,
            shift,
            alt,
        }
    }

    fn any(self) -> bool {
        self.command || self.control || self.shift || self.alt
    }
}

/// A bare modifier key press on its own (mirrors `modifierToShortcut()`).
/// The names are the ones `modifier_shortcut::NATIVE_SHORTCUTS` stores, where
/// `Meta` is the Command key.
///
/// `KeyCode` is winit's `#[non_exhaustive]` set of about 190 physical keys:
/// listing every key that is not a modifier would be noise, not safety.
#[allow(clippy::wildcard_enum_match_arm)]
pub fn modifier_only_shortcut(key: KeyCode) -> Option<&'static str> {
    Some(match key {
        KeyCode::SuperLeft => "MetaLeft",
        KeyCode::SuperRight => "MetaRight",
        KeyCode::ControlLeft => "ControlLeft",
        KeyCode::ControlRight => "ControlRight",
        KeyCode::AltLeft => "AltLeft",
        KeyCode::AltRight => "AltRight",
        KeyCode::ShiftLeft => "ShiftLeft",
        KeyCode::ShiftRight => "ShiftRight",
        _ => return None,
    })
}

/// The accelerator key name `global_hotkey::HotKey::from_str` accepts for a
/// physical key, or `None` for a key it cannot register (the ISO `§`/`<` key,
/// the JIS `¥`/`ろ`/英数/かな keys, Fn, Caps Lock, media keys). Letters and
/// digits keep the short `D`/`1` form bindings were already stored in.
///
/// Same open set as `modifier_only_shortcut`: the wildcard is every key
/// `global-hotkey` has no name for.
#[allow(clippy::wildcard_enum_match_arm)]
fn key_name(key: KeyCode) -> Option<&'static str> {
    Some(match key {
        KeyCode::KeyA => "A",
        KeyCode::KeyB => "B",
        KeyCode::KeyC => "C",
        KeyCode::KeyD => "D",
        KeyCode::KeyE => "E",
        KeyCode::KeyF => "F",
        KeyCode::KeyG => "G",
        KeyCode::KeyH => "H",
        KeyCode::KeyI => "I",
        KeyCode::KeyJ => "J",
        KeyCode::KeyK => "K",
        KeyCode::KeyL => "L",
        KeyCode::KeyM => "M",
        KeyCode::KeyN => "N",
        KeyCode::KeyO => "O",
        KeyCode::KeyP => "P",
        KeyCode::KeyQ => "Q",
        KeyCode::KeyR => "R",
        KeyCode::KeyS => "S",
        KeyCode::KeyT => "T",
        KeyCode::KeyU => "U",
        KeyCode::KeyV => "V",
        KeyCode::KeyW => "W",
        KeyCode::KeyX => "X",
        KeyCode::KeyY => "Y",
        KeyCode::KeyZ => "Z",
        KeyCode::Digit0 => "0",
        KeyCode::Digit1 => "1",
        KeyCode::Digit2 => "2",
        KeyCode::Digit3 => "3",
        KeyCode::Digit4 => "4",
        KeyCode::Digit5 => "5",
        KeyCode::Digit6 => "6",
        KeyCode::Digit7 => "7",
        KeyCode::Digit8 => "8",
        KeyCode::Digit9 => "9",
        KeyCode::Backquote => "Backquote",
        KeyCode::Minus => "Minus",
        KeyCode::Equal => "Equal",
        KeyCode::BracketLeft => "BracketLeft",
        KeyCode::BracketRight => "BracketRight",
        KeyCode::Backslash => "Backslash",
        KeyCode::Semicolon => "Semicolon",
        KeyCode::Quote => "Quote",
        KeyCode::Comma => "Comma",
        KeyCode::Period => "Period",
        KeyCode::Slash => "Slash",
        KeyCode::Space => "Space",
        KeyCode::Enter => "Enter",
        KeyCode::Tab => "Tab",
        KeyCode::ArrowUp => "ArrowUp",
        KeyCode::ArrowDown => "ArrowDown",
        KeyCode::ArrowLeft => "ArrowLeft",
        KeyCode::ArrowRight => "ArrowRight",
        KeyCode::Home => "Home",
        KeyCode::End => "End",
        KeyCode::PageUp => "PageUp",
        KeyCode::PageDown => "PageDown",
        KeyCode::Numpad0 => "Numpad0",
        KeyCode::Numpad1 => "Numpad1",
        KeyCode::Numpad2 => "Numpad2",
        KeyCode::Numpad3 => "Numpad3",
        KeyCode::Numpad4 => "Numpad4",
        KeyCode::Numpad5 => "Numpad5",
        KeyCode::Numpad6 => "Numpad6",
        KeyCode::Numpad7 => "Numpad7",
        KeyCode::Numpad8 => "Numpad8",
        KeyCode::Numpad9 => "Numpad9",
        KeyCode::NumpadAdd => "NumpadAdd",
        KeyCode::NumpadSubtract => "NumpadSubtract",
        KeyCode::NumpadMultiply => "NumpadMultiply",
        KeyCode::NumpadDivide => "NumpadDivide",
        KeyCode::NumpadDecimal => "NumpadDecimal",
        KeyCode::NumpadEqual => "NumpadEqual",
        KeyCode::NumpadEnter => "NumpadEnter",
        KeyCode::F1 => "F1",
        KeyCode::F2 => "F2",
        KeyCode::F3 => "F3",
        KeyCode::F4 => "F4",
        KeyCode::F5 => "F5",
        KeyCode::F6 => "F6",
        KeyCode::F7 => "F7",
        KeyCode::F8 => "F8",
        KeyCode::F9 => "F9",
        KeyCode::F10 => "F10",
        KeyCode::F11 => "F11",
        KeyCode::F12 => "F12",
        KeyCode::F13 => "F13",
        KeyCode::F14 => "F14",
        KeyCode::F15 => "F15",
        KeyCode::F16 => "F16",
        KeyCode::F17 => "F17",
        KeyCode::F18 => "F18",
        KeyCode::F19 => "F19",
        KeyCode::F20 => "F20",
        _ => return None,
    })
}

fn is_function_key(key: KeyCode) -> bool {
    matches!(
        key,
        KeyCode::F1
            | KeyCode::F2
            | KeyCode::F3
            | KeyCode::F4
            | KeyCode::F5
            | KeyCode::F6
            | KeyCode::F7
            | KeyCode::F8
            | KeyCode::F9
            | KeyCode::F10
            | KeyCode::F11
            | KeyCode::F12
            | KeyCode::F13
            | KeyCode::F14
            | KeyCode::F15
            | KeyCode::F16
            | KeyCode::F17
            | KeyCode::F18
            | KeyCode::F19
            | KeyCode::F20
    )
}

/// True for a bare letter/digit/symbol key with no modifier and no function
/// key held (mirrors `shortcutMissingModifier()`).
pub fn missing_modifier(key: KeyCode, modifiers: Modifiers) -> bool {
    !modifiers.any() && !is_function_key(key)
}

/// Builds the accelerator string the backend stores from a non-modifier key
/// press plus the modifiers held at the same time (mirrors
/// `keyEventToShortcut()`). Command stays `CommandOrControl`, the token
/// existing bindings use; the physical Control key is `Control`, so ⌃Space no
/// longer collapses into ⌘Space. Returns `None` for a bare modifier press or
/// a key `global-hotkey` cannot register.
pub fn format_combo(key: KeyCode, modifiers: Modifiers) -> Option<String> {
    if modifier_only_shortcut(key).is_some() {
        return None;
    }
    let key = key_name(key)?;

    let mut parts = Vec::new();
    if modifiers.command {
        parts.push("CommandOrControl");
    }
    if modifiers.control {
        parts.push("Control");
    }
    if modifiers.shift {
        parts.push("Shift");
    }
    if modifiers.alt {
        parts.push("Alt");
    }
    parts.push(key);
    Some(parts.join("+"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What Slint reports on macOS for a physical key set, Command and
    /// Control swapped (see `Modifiers::from_slint`).
    fn slint(command: bool, control: bool, shift: bool, alt: bool) -> Modifiers {
        Modifiers::from_slint(command, shift, alt, control)
    }

    #[test]
    fn slint_modifiers_are_swapped_back_to_physical_keys() {
        let command = slint(true, false, false, false);
        assert!(command.command && !command.control);
        let control = slint(false, true, false, false);
        assert!(control.control && !control.command);
    }

    #[test]
    fn bare_letter_without_modifier_has_no_modifier() {
        assert_eq!(
            format_combo(KeyCode::KeyD, Modifiers::default()),
            Some("D".to_string())
        );
        assert!(missing_modifier(KeyCode::KeyD, Modifiers::default()));
    }

    #[test]
    fn function_key_does_not_need_a_modifier() {
        assert!(!missing_modifier(KeyCode::F5, Modifiers::default()));
        assert!(!missing_modifier(KeyCode::F13, Modifiers::default()));
        assert_eq!(
            format_combo(KeyCode::F5, Modifiers::default()),
            Some("F5".to_string())
        );
    }

    #[test]
    fn combo_with_command_and_shift() {
        assert_eq!(
            format_combo(KeyCode::KeyD, slint(true, false, true, false)),
            Some("CommandOrControl+Shift+D".to_string())
        );
    }

    /// #360: ⌃Space was saved as `CommandOrControl+Space`, i.e. ⌘Space.
    #[test]
    fn control_space_is_control_not_command() {
        assert_eq!(
            format_combo(KeyCode::Space, slint(false, true, false, false)),
            Some("Control+Space".to_string())
        );
    }

    #[test]
    fn command_space_stays_command_or_control() {
        assert_eq!(
            format_combo(KeyCode::Space, slint(true, false, false, false)),
            Some("CommandOrControl+Space".to_string())
        );
    }

    /// #360: ⌃⌥⇧⌘T dropped Control and never matched.
    #[test]
    fn hyper_keeps_both_command_and_control() {
        assert_eq!(
            format_combo(KeyCode::KeyT, slint(true, true, true, true)),
            Some("CommandOrControl+Control+Shift+Alt+T".to_string())
        );
    }

    #[test]
    fn punctuation_is_named_after_the_physical_key() {
        assert_eq!(
            format_combo(KeyCode::Minus, slint(false, false, true, false)),
            Some("Shift+Minus".to_string())
        );
    }

    /// #360: left Command was `ControlLeft`, left Control was `MetaLeft`,
    /// right Option was `AltLeft`.
    #[test]
    fn bare_modifiers_keep_their_side() {
        for (key, name) in [
            (KeyCode::SuperLeft, "MetaLeft"),
            (KeyCode::SuperRight, "MetaRight"),
            (KeyCode::ControlLeft, "ControlLeft"),
            (KeyCode::ControlRight, "ControlRight"),
            (KeyCode::AltLeft, "AltLeft"),
            (KeyCode::AltRight, "AltRight"),
            (KeyCode::ShiftLeft, "ShiftLeft"),
            (KeyCode::ShiftRight, "ShiftRight"),
        ] {
            assert_eq!(modifier_only_shortcut(key), Some(name), "{key:?}");
            assert_eq!(format_combo(key, Modifiers::default()), None, "{key:?}");
        }
    }

    /// The captured key is the physical one, so the layout the user types
    /// with does not change what is stored. AZERTY: the key labelled A is
    /// the QWERTY Q position and `&` sits on Digit1. QWERTZ: Z and Y are
    /// swapped. Russian: the key producing `в` is the D position. Before
    /// #360 these were stored from the produced character (`A`, `7`, `Y`,
    /// nothing).
    #[test]
    fn non_qwerty_layouts_store_the_pressed_position() {
        let command = slint(true, false, false, false);
        for (key, stored) in [
            (KeyCode::KeyQ, "CommandOrControl+Q"),
            (KeyCode::Digit1, "CommandOrControl+1"),
            (KeyCode::KeyY, "CommandOrControl+Y"),
            (KeyCode::KeyD, "CommandOrControl+D"),
        ] {
            assert_eq!(format_combo(key, command).as_deref(), Some(stored));
        }
    }

    #[test]
    fn unregistrable_keys_have_no_combo() {
        let command = slint(true, false, false, false);
        for key in [
            KeyCode::IntlBackslash,
            KeyCode::IntlYen,
            KeyCode::IntlRo,
            KeyCode::Lang1,
            KeyCode::Fn,
            KeyCode::CapsLock,
        ] {
            assert_eq!(format_combo(key, command), None, "{key:?}");
        }
    }

    #[test]
    fn physical_key_is_consumed_once() {
        LAST_PHYSICAL_KEY.with(|slot| slot.set(Some(KeyCode::KeyD)));
        assert_eq!(take_physical_key(), Some(KeyCode::KeyD));
        assert_eq!(take_physical_key(), None);
    }
}
