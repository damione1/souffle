//! Shortcut labels as the user sees them (#360), and the four window
//! properties that show one.
//!
//! A label depends on the keyboard layout (`keyboard_layout.rs`), which can
//! change while the app runs. Every label is set through `project`, which
//! remembers the accelerator behind it, so `refresh` can re-render them all
//! when the window regains focus after a layout switch. Opening Settings
//! reloads the bindings and projects them again anyway.

use std::cell::RefCell;

use objc2::MainThreadMarker;

use crate::MainWindow;
use crate::keyboard_layout;

/// A window property that shows a shortcut label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelSlot {
    SettingsToggle,
    SettingsPtt,
    /// The home screen's "press … to dictate" hint.
    DictationHint,
    OnboardingToggle,
}

impl LabelSlot {
    const ALL: [LabelSlot; 4] = [
        LabelSlot::SettingsToggle,
        LabelSlot::SettingsPtt,
        LabelSlot::DictationHint,
        LabelSlot::OnboardingToggle,
    ];

    fn index(self) -> usize {
        match self {
            LabelSlot::SettingsToggle => 0,
            LabelSlot::SettingsPtt => 1,
            LabelSlot::DictationHint => 2,
            LabelSlot::OnboardingToggle => 3,
        }
    }

    fn set(self, window: &MainWindow, label: &str) {
        match self {
            LabelSlot::SettingsToggle => window.set_settings_toggle_shortcut_label(label.into()),
            LabelSlot::SettingsPtt => window.set_settings_ptt_shortcut_label(label.into()),
            LabelSlot::DictationHint => window.set_dictation_shortcut(label.into()),
            LabelSlot::OnboardingToggle => {
                window.set_onboarding_toggle_shortcut_label(label.into())
            }
        }
    }
}

thread_local! {
    /// The accelerator each slot currently shows. UI thread only.
    static PROJECTED: RefCell<[Option<String>; 4]> = const { RefCell::new([None, None, None, None]) };
}

/// Shows `accelerator` in `slot` and remembers it for `refresh`.
pub fn project(window: &MainWindow, slot: LabelSlot, accelerator: &str) {
    PROJECTED.with(|projected| {
        projected.borrow_mut()[slot.index()] = Some(accelerator.to_string());
    });
    slot.set(window, &format(accelerator));
}

/// Re-renders every projected label, so a keyboard layout switched while
/// the app was in the background shows up.
pub fn refresh(window: &MainWindow) {
    let projected = PROJECTED.with(|projected| projected.borrow().clone());
    for slot in LabelSlot::ALL {
        if let Some(accelerator) = &projected[slot.index()] {
            slot.set(window, &format(accelerator));
        }
    }
}

/// The label for a stored accelerator. Character keys are named after what
/// they are labelled on the current layout when this runs on the main
/// thread (the only thread the layout lookup may run on); anywhere else, or
/// when the lookup fails, they keep their US name.
pub fn format(accelerator: &str) -> String {
    format_with(accelerator, |token| {
        MainThreadMarker::new().and_then(|mtm| keyboard_layout::key_label(mtm, token))
    })
}

/// `format` with the layout lookup passed in, so the token mapping is
/// testable without a keyboard layout.
fn format_with(accelerator: &str, layout_label: impl Fn(&str) -> Option<String>) -> String {
    if accelerator.is_empty() {
        return String::new();
    }
    accelerator
        .split('+')
        .map(|token| match symbol(token) {
            Some(symbol) => symbol.to_string(),
            None => layout_label(token).unwrap_or_else(|| token.to_string()),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A modifier token as its macOS symbol, and a bare modifier with its side
/// ("⌘ Left"), the same way for all four modifiers.
fn symbol(token: &str) -> Option<&'static str> {
    Some(match token {
        "CommandOrControl" => "\u{2318}",
        "Control" => "\u{2303}",
        "Shift" => "\u{21e7}",
        "Alt" => "\u{2325}",
        "MetaLeft" => "\u{2318} Left",
        "MetaRight" => "\u{2318} Right",
        "ControlLeft" => "\u{2303} Left",
        "ControlRight" => "\u{2303} Right",
        "AltLeft" => "\u{2325} Left",
        "AltRight" => "\u{2325} Right",
        "ShiftLeft" => "\u{21e7} Left",
        "ShiftRight" => "\u{21e7} Right",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The US names, as with no layout lookup (off the main thread).
    fn us(accelerator: &str) -> String {
        format_with(accelerator, |_| None)
    }

    #[test]
    fn every_modifier_shows_its_symbol_and_side() {
        for (stored, label) in [
            ("MetaLeft", "⌘ Left"),
            ("MetaRight", "⌘ Right"),
            ("ControlLeft", "⌃ Left"),
            ("ControlRight", "⌃ Right"),
            ("AltLeft", "⌥ Left"),
            ("AltRight", "⌥ Right"),
            ("ShiftLeft", "⇧ Left"),
            ("ShiftRight", "⇧ Right"),
            ("CommandOrControl+Shift+Space", "⌘ ⇧ Space"),
            ("Control+Space", "⌃ Space"),
            ("CommandOrControl+Control+Shift+Alt+T", "⌘ ⌃ ⇧ ⌥ T"),
            ("F5", "F5"),
            ("", ""),
        ] {
            assert_eq!(us(stored), label, "{stored}");
        }
    }

    /// AZERTY: the key stored as `Q` is labelled A, `1` types `&`. Named keys
    /// and modifiers never go through the layout.
    #[test]
    fn character_keys_take_the_layout_label() {
        let azerty = |token: &str| match token {
            "Q" => Some("A".to_string()),
            "1" => Some("&".to_string()),
            other => panic!("layout asked for {other}"),
        };
        assert_eq!(format_with("CommandOrControl+Q", azerty), "⌘ A");
        assert_eq!(format_with("Control+Shift+1", azerty), "⌃ ⇧ &");
        assert_eq!(format_with("CommandOrControl+Space", |_| None), "⌘ Space");
    }

    #[test]
    fn a_failed_lookup_keeps_the_us_name() {
        assert_eq!(format_with("CommandOrControl+Minus", |_| None), "⌘ Minus");
    }
}
