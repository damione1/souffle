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
//! `take_pressed_key` in the capture callback reads the key of that event.
//!
//! One correction on top of winit 0.30: it maps both the ANSI grave key
//! (keyCode `0x32`, `<` next to left Shift on ISO keyboards) and the ISO
//! section key (`0x0A`, top left: `@` on French AZERTY, `§`/`^` on QWERTZ) to
//! `Backquote` (winit PR #4019 fixed it after 0.30). Storing `Backquote` for
//! the ISO key would register `0x32`, a different key, so the raw keyCode of
//! the event being dispatched decides between the two (`resolve_key`).

use std::cell::Cell;

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSEventType};
use slint::winit_030::winit::event::KeyEvent;
use slint::winit_030::winit::keyboard::{KeyCode, PhysicalKey};

/// `kVK_ISO_Section` (HIToolbox `Events.h`).
const ISO_SECTION_KEYCODE: u16 = 0x0a;
/// `kVK_ANSI_Grave`.
const ANSI_GRAVE_KEYCODE: u16 = 0x32;
/// `kVK_CapsLock` and `kVK_Function` (Fn/Globe): winit reports neither as a
/// keyboard event, so the recorder learns about them from an AppKit monitor
/// (`is_unusable_flags_key`).
const CAPS_LOCK_KEYCODE: u16 = 0x39;
const FUNCTION_KEYCODE: u16 = 0x3f;

/// The physical key of a keyboard event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressedKey {
    Code(KeyCode),
    /// A key winit has no `KeyCode` for (the JIS ろ/英数/かな keys on
    /// macOS). It cannot be stored, but it was pressed.
    Unidentified,
}

impl PressedKey {
    fn code(self) -> Option<KeyCode> {
        match self {
            PressedKey::Code(code) => Some(code),
            PressedKey::Unidentified => None,
        }
    }
}

thread_local! {
    /// Physical key of the last winit keyboard event, consumed by the capture
    /// callback that event triggers. UI thread only.
    static LAST_PRESSED_KEY: Cell<Option<PressedKey>> = const { Cell::new(None) };
}

/// Records the physical key of a winit keyboard event (press or release).
pub fn observe_key_event(event: &KeyEvent) {
    let raw = match event.physical_key {
        PhysicalKey::Code(KeyCode::Backquote) => current_event_key_code(),
        PhysicalKey::Code(_) | PhysicalKey::Unidentified(_) => None,
    };
    LAST_PRESSED_KEY.with(|slot| slot.set(Some(resolve_key(event.physical_key, raw))));
}

/// Undoes winit 0.30's `0x0A` → `Backquote` fold. `raw` is the macOS keyCode
/// of the event being dispatched, when it could be read.
fn resolve_key(physical: PhysicalKey, raw: Option<u16>) -> PressedKey {
    match physical {
        PhysicalKey::Code(KeyCode::Backquote) if raw == Some(ISO_SECTION_KEYCODE) => {
            PressedKey::Code(KeyCode::IntlBackslash)
        }
        PhysicalKey::Code(code) => PressedKey::Code(code),
        PhysicalKey::Unidentified(_) => PressedKey::Unidentified,
    }
}

/// The keyCode of AppKit's current event, read only to tell the two keys
/// winit folds into `Backquote` apart.
///
/// During normal dispatch the winit hook runs inside `-[NSView keyDown:]`,
/// so `currentEvent` is the event being handled. winit queues an event
/// instead when its handler is already running (re-entrance), and then
/// `currentEvent` may be a later one; anything that is not a key event with
/// one of the two folded keyCodes is therefore ignored and winit's
/// `Backquote` stands, which is the behaviour before this check.
fn current_event_key_code() -> Option<u16> {
    let mtm = MainThreadMarker::new()?;
    let event = NSApplication::sharedApplication(mtm).currentEvent()?;
    let kind = event.r#type();
    if kind != NSEventType::KeyDown && kind != NSEventType::KeyUp {
        return None;
    }
    let code = event.keyCode();
    (code == ISO_SECTION_KEYCODE || code == ANSI_GRAVE_KEYCODE).then_some(code)
}

/// The physical key of the event being dispatched, taken so a later Slint
/// event that did not come through winit cannot reuse it.
pub fn take_pressed_key() -> Option<PressedKey> {
    LAST_PRESSED_KEY.with(Cell::take)
}

/// True for the keyCode of a `flagsChanged` event from Caps Lock or Fn/Globe,
/// the two keys that never reach the recorder as a key press.
pub fn is_unusable_flags_key(key_code: u16) -> bool {
    key_code == CAPS_LOCK_KEYCODE || key_code == FUNCTION_KEYCODE
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
fn modifier_only_shortcut(key: KeyCode) -> Option<&'static str> {
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

/// Keys whose press types a character (letters, digits, punctuation): the
/// ones whose label comes from the keyboard layout.
fn types_character(key: KeyCode) -> bool {
    key_name(key).is_some_and(|name| crate::keyboard_layout::virtual_key_code(name).is_some())
}

/// Why a key press is not saved as a shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// A letter/digit/symbol with no modifier and no function key.
    MissingModifier,
    /// A key `global-hotkey` cannot register (ISO section key, JIS keys,
    /// media keys, or one winit cannot identify).
    UnusableKey,
}

/// What the recorder does with a key press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capture {
    /// A bare modifier: saved on its release, unless a real key comes first.
    Modifier(&'static str),
    /// An accelerator to save. `swallows_character` when only ⌥ (and ⇧) are
    /// held with a character key: the hotkey then eats that character (⌥⇧L
    /// is `|` on AZERTY) in every app, which is allowed but worth a warning.
    Save {
        accelerator: String,
        swallows_character: bool,
    },
    Rejected(Rejection),
}

/// Decides what a key press means for the recorder (mirrors
/// `keyEventToShortcut()` and `shortcutMissingModifier()`). Command stays
/// `CommandOrControl`, the token existing bindings use; the physical Control
/// key is `Control`, so ⌃Space no longer collapses into ⌘Space.
pub fn classify(key: PressedKey, modifiers: Modifiers) -> Capture {
    let Some(code) = key.code() else {
        return Capture::Rejected(Rejection::UnusableKey);
    };
    if let Some(name) = modifier_only_shortcut(code) {
        return Capture::Modifier(name);
    }
    let Some(name) = key_name(code) else {
        return Capture::Rejected(Rejection::UnusableKey);
    };
    if !modifiers.any() && !is_function_key(code) {
        return Capture::Rejected(Rejection::MissingModifier);
    }

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
    parts.push(name);
    Capture::Save {
        accelerator: parts.join("+"),
        swallows_character: modifiers.alt
            && !modifiers.command
            && !modifiers.control
            && types_character(code),
    }
}

/// The bare modifier a key release ends, if any.
pub fn released_modifier(key: PressedKey) -> Option<&'static str> {
    key.code().and_then(modifier_only_shortcut)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What Slint reports on macOS for a physical key set, Command and
    /// Control swapped (see `Modifiers::from_slint`).
    fn slint(command: bool, control: bool, shift: bool, alt: bool) -> Modifiers {
        Modifiers::from_slint(command, shift, alt, control)
    }

    fn save(accelerator: &str) -> Capture {
        Capture::Save {
            accelerator: accelerator.to_string(),
            swallows_character: false,
        }
    }

    fn key(code: KeyCode) -> PressedKey {
        PressedKey::Code(code)
    }

    const COMMAND: Modifiers = Modifiers {
        command: true,
        control: false,
        shift: false,
        alt: false,
    };

    #[test]
    fn slint_modifiers_are_swapped_back_to_physical_keys() {
        let command = slint(true, false, false, false);
        assert!(command.command && !command.control);
        let control = slint(false, true, false, false);
        assert!(control.control && !control.command);
    }

    #[test]
    fn bare_letter_needs_a_modifier() {
        assert_eq!(
            classify(key(KeyCode::KeyD), Modifiers::default()),
            Capture::Rejected(Rejection::MissingModifier)
        );
    }

    #[test]
    fn function_key_does_not_need_a_modifier() {
        assert_eq!(classify(key(KeyCode::F5), Modifiers::default()), save("F5"));
        assert_eq!(
            classify(key(KeyCode::F13), Modifiers::default()),
            save("F13")
        );
    }

    #[test]
    fn combo_with_command_and_shift() {
        assert_eq!(
            classify(key(KeyCode::KeyD), slint(true, false, true, false)),
            save("CommandOrControl+Shift+D")
        );
    }

    /// #360: ⌃Space was saved as `CommandOrControl+Space`, i.e. ⌘Space.
    #[test]
    fn control_space_is_control_not_command() {
        assert_eq!(
            classify(key(KeyCode::Space), slint(false, true, false, false)),
            save("Control+Space")
        );
    }

    #[test]
    fn command_space_stays_command_or_control() {
        assert_eq!(
            classify(key(KeyCode::Space), COMMAND),
            save("CommandOrControl+Space")
        );
    }

    /// #360: ⌃⌥⇧⌘T dropped Control and never matched.
    #[test]
    fn hyper_keeps_both_command_and_control() {
        assert_eq!(
            classify(key(KeyCode::KeyT), slint(true, true, true, true)),
            save("CommandOrControl+Control+Shift+Alt+T")
        );
    }

    #[test]
    fn punctuation_is_named_after_the_physical_key() {
        assert_eq!(
            classify(key(KeyCode::Minus), slint(true, false, true, false)),
            save("CommandOrControl+Shift+Minus")
        );
    }

    /// #360: left Command was `ControlLeft`, left Control was `MetaLeft`,
    /// right Option was `AltLeft`.
    #[test]
    fn bare_modifiers_keep_their_side() {
        for (code, name) in [
            (KeyCode::SuperLeft, "MetaLeft"),
            (KeyCode::SuperRight, "MetaRight"),
            (KeyCode::ControlLeft, "ControlLeft"),
            (KeyCode::ControlRight, "ControlRight"),
            (KeyCode::AltLeft, "AltLeft"),
            (KeyCode::AltRight, "AltRight"),
            (KeyCode::ShiftLeft, "ShiftLeft"),
            (KeyCode::ShiftRight, "ShiftRight"),
        ] {
            assert_eq!(
                classify(key(code), Modifiers::default()),
                Capture::Modifier(name)
            );
            assert_eq!(released_modifier(key(code)), Some(name), "{code:?}");
        }
        assert_eq!(released_modifier(key(KeyCode::KeyA)), None);
        assert_eq!(released_modifier(PressedKey::Unidentified), None);
    }

    /// The captured key is the physical one, so the layout the user types
    /// with does not change what is stored. AZERTY: the key labelled A is
    /// the QWERTY Q position and `&` sits on Digit1. QWERTZ: Z and Y are
    /// swapped. Russian: the key producing `в` is the D position. Before
    /// #360 these were stored from the produced character (`A`, `7`, `Y`,
    /// nothing).
    #[test]
    fn non_qwerty_layouts_store_the_pressed_position() {
        for (code, stored) in [
            (KeyCode::KeyQ, "CommandOrControl+Q"),
            (KeyCode::Digit1, "CommandOrControl+1"),
            (KeyCode::KeyY, "CommandOrControl+Y"),
            (KeyCode::KeyD, "CommandOrControl+D"),
        ] {
            assert_eq!(classify(key(code), COMMAND), save(stored));
        }
    }

    /// global-hotkey 0.6.4 has no name for these keys, so they are refused
    /// with a message instead of being saved as another key or ignored.
    #[test]
    fn unregistrable_keys_are_rejected() {
        for pressed in [
            key(KeyCode::IntlBackslash),
            key(KeyCode::IntlYen),
            key(KeyCode::IntlRo),
            key(KeyCode::Lang1),
            key(KeyCode::Lang2),
            key(KeyCode::MediaPlayPause),
            PressedKey::Unidentified,
        ] {
            for modifiers in [COMMAND, Modifiers::default()] {
                assert_eq!(
                    classify(pressed, modifiers),
                    Capture::Rejected(Rejection::UnusableKey),
                    "{pressed:?}"
                );
            }
        }
    }

    /// #360 follow-up: winit 0.30 reports the ISO section key (`0x0A`) as
    /// `Backquote`; stored that way it would register `0x32`, another key.
    #[test]
    fn iso_section_key_never_becomes_backquote() {
        let backquote = PhysicalKey::Code(KeyCode::Backquote);
        assert_eq!(
            resolve_key(backquote, Some(ISO_SECTION_KEYCODE)),
            key(KeyCode::IntlBackslash)
        );
        assert_eq!(
            classify(resolve_key(backquote, Some(ISO_SECTION_KEYCODE)), COMMAND),
            Capture::Rejected(Rejection::UnusableKey)
        );
    }

    #[test]
    fn ansi_grave_key_stays_backquote() {
        let backquote = PhysicalKey::Code(KeyCode::Backquote);
        for raw in [Some(ANSI_GRAVE_KEYCODE), None] {
            assert_eq!(
                resolve_key(backquote, raw),
                key(KeyCode::Backquote),
                "{raw:?}"
            );
        }
        assert_eq!(
            classify(resolve_key(backquote, Some(ANSI_GRAVE_KEYCODE)), COMMAND),
            save("CommandOrControl+Backquote")
        );
    }

    /// Only the `Backquote` fold is corrected: a raw `0x0A` next to another
    /// winit key is a stale `currentEvent`, not a reason to change the key.
    #[test]
    fn raw_keycode_only_overrides_backquote() {
        assert_eq!(
            resolve_key(PhysicalKey::Code(KeyCode::KeyA), Some(ISO_SECTION_KEYCODE)),
            key(KeyCode::KeyA)
        );
    }

    #[test]
    fn caps_lock_and_fn_are_the_unusable_flags_keys() {
        assert!(is_unusable_flags_key(0x39));
        assert!(is_unusable_flags_key(0x3f));
        for modifier in [0x37, 0x36, 0x38, 0x3c, 0x3a, 0x3d, 0x3b, 0x3e] {
            assert!(!is_unusable_flags_key(modifier), "{modifier:#x}");
        }
    }

    /// ⌥ or ⌥⇧ plus a character key eats that character in every app.
    #[test]
    fn option_only_character_combos_warn() {
        let option = slint(false, false, false, true);
        let option_shift = slint(false, false, true, true);
        for (code, modifiers, accelerator) in [
            (KeyCode::KeyL, option_shift, "Shift+Alt+L"),
            (KeyCode::Digit5, option, "Alt+5"),
            (KeyCode::Minus, option, "Alt+Minus"),
        ] {
            assert_eq!(
                classify(key(code), modifiers),
                Capture::Save {
                    accelerator: accelerator.to_string(),
                    swallows_character: true,
                }
            );
        }
    }

    #[test]
    fn option_with_command_control_or_a_named_key_does_not_warn() {
        let option = slint(false, false, false, true);
        for (code, modifiers, accelerator) in [
            (
                KeyCode::KeyL,
                slint(true, false, false, true),
                "CommandOrControl+Alt+L",
            ),
            (
                KeyCode::KeyL,
                slint(false, true, false, true),
                "Control+Alt+L",
            ),
            (KeyCode::Space, option, "Alt+Space"),
            (KeyCode::F5, option, "Alt+F5"),
            (KeyCode::KeyL, slint(false, false, true, false), "Shift+L"),
        ] {
            assert_eq!(classify(key(code), modifiers), save(accelerator));
        }
    }

    #[test]
    fn physical_key_is_consumed_once() {
        LAST_PRESSED_KEY.with(|slot| slot.set(Some(key(KeyCode::KeyD))));
        assert_eq!(take_pressed_key(), Some(key(KeyCode::KeyD)));
        assert_eq!(take_pressed_key(), None);
    }
}
