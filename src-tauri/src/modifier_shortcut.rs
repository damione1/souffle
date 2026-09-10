use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use core_foundation::base::TCFType;
use core_graphics::event::{
    CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult,
};
use tauri::{AppHandle, Manager};
use tauri_specta::Event;
use tracing::{error, info};

use crate::app_events::{ModifierTapStatus, ShortcutPttStart, ShortcutPttStop, ShortcutToggle};
use crate::state::AppState;

/// Last `ModifierTapStatus` emitted. The event is edge-triggered (install
/// success/failure), so a webview that reloads has nothing to listen to until
/// the next retry; bootstrap reads this snapshot instead (SOU-116).
static MODIFIER_TAP_STATUS: Mutex<Option<ModifierTapStatus>> = Mutex::new(None);

/// Snapshot for `commands::get_modifier_tap_status`. A read, never a rebuild.
pub fn modifier_tap_status() -> Option<ModifierTapStatus> {
    MODIFIER_TAP_STATUS.lock().ok().and_then(|guard| *guard)
}

fn store_and_emit_modifier_tap_status(app: &AppHandle, installed: bool) {
    let status = ModifierTapStatus { installed };
    if let Ok(mut guard) = MODIFIER_TAP_STATUS.lock() {
        *guard = Some(status);
    }
    let _ = status.emit(app);
}

/// Where a dictation shortcut is registered (SOU-032 / SOU-115).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortcutRegistrationTarget {
    /// Empty binding — nothing to register.
    None,
    /// Single native key handled by the CGEventTap.
    Native,
    /// Classic combo handled by `tauri-plugin-global-shortcut`.
    Plugin,
}

/// Single-key bindings that the plugin cannot register. Combos stay on
/// `tauri-plugin-global-shortcut` (SOU-032). Shared by Toggle and PTT (SOU-115).
pub(crate) fn is_native_shortcut(shortcut: &str) -> bool {
    matches!(
        shortcut,
        "Fn" | "MetaLeft"
            | "MetaRight"
            | "ShiftLeft"
            | "ShiftRight"
            | "AltLeft"
            | "AltRight"
            | "ControlLeft"
            | "ControlRight"
            | "F5"
            | "F6"
            | "F7"
            | "F8"
            | "F9"
            | "F10"
            | "F11"
            | "F12"
    )
}

/// Route a stored accelerator to the native tap or the global-shortcut plugin.
pub(crate) fn shortcut_registration_target(shortcut: &str) -> ShortcutRegistrationTarget {
    if shortcut.is_empty() {
        ShortcutRegistrationTarget::None
    } else if is_native_shortcut(shortcut) {
        ShortcutRegistrationTarget::Native
    } else {
        ShortcutRegistrationTarget::Plugin
    }
}

pub fn start_modifier_tap(app: AppHandle) {
    thread::spawn(move || {
        // Wait a little before starting to ensure AppState is registered.
        // Status is not emitted during this window so the settings banner
        // cannot flash on every launch (SOU-116 risk zone).
        thread::sleep(Duration::from_millis(500));
        let mut warned = false;
        loop {
            if install_and_run(app.clone()) {
                return;
            }
            store_and_emit_modifier_tap_status(&app, false);
            if !warned {
                error!(
                    "Failed to install modifier CGEventTap. Is Accessibility granted? Retrying."
                );
                warned = true;
            }
            thread::sleep(Duration::from_secs(2));
        }
    });
}

fn install_and_run(app: AppHandle) -> bool {
    let tap_port_ptr = Arc::new(AtomicUsize::new(0));
    let tap_port_clone = tap_port_ptr.clone();
    // Callback takes ownership of `app`; keep a handle for status emits after
    // install succeeds or the runloop source fails to attach.
    let app_for_status = app.clone();

    let tap_result = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        vec![
            CGEventType::FlagsChanged,
            CGEventType::KeyDown,
            CGEventType::KeyUp,
        ],
        move |_proxy, event_type, event| {
            if matches!(
                event_type,
                CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
            ) {
                let ptr = tap_port_clone.load(Ordering::Relaxed);
                if ptr != 0 {
                    unsafe {
                        // CGEventTapEnable takes a CFMachPortRef.
                        unsafe extern "C" {
                            fn CGEventTapEnable(tap: *const std::ffi::c_void, enable: bool);
                        }
                        CGEventTapEnable(ptr as *const _, true);
                    }
                }
                return CallbackResult::Keep;
            }

            let state = match app.try_state::<AppState>() {
                Some(s) => s.inner(),
                None => return CallbackResult::Keep,
            };
            let toggle_shortcut = {
                let lock = state.modifier_toggle_shortcut.read().unwrap();
                lock.clone()
            };
            let ptt_shortcut = {
                let lock = state.modifier_ptt_shortcut.read().unwrap();
                lock.clone()
            };

            let keycode = event
                .get_integer_value_field(core_graphics::event::EventField::KEYBOARD_EVENT_KEYCODE);
            let flags = event.get_flags();

            let matches_toggle = toggle_shortcut
                .as_deref()
                .is_some_and(|s| shortcut_matches_keycode(s, keycode));
            let matches_ptt = ptt_shortcut
                .as_deref()
                .is_some_and(|s| shortcut_matches_keycode(s, keycode));

            if !matches_toggle && !matches_ptt {
                return CallbackResult::Keep;
            }

            if matches!(event_type, CGEventType::FlagsChanged) {
                let is_pressed = match keycode {
                    55 | 54 => flags.contains(CGEventFlags::CGEventFlagCommand),
                    56 | 60 => flags.contains(CGEventFlags::CGEventFlagShift),
                    58 | 61 => flags.contains(CGEventFlags::CGEventFlagAlternate),
                    59 | 62 => flags.contains(CGEventFlags::CGEventFlagControl),
                    63 => flags.contains(CGEventFlags::CGEventFlagSecondaryFn),
                    _ => false,
                };

                if is_pressed {
                    if matches_toggle {
                        emit_toggle(state, &app);
                    }
                    if matches_ptt {
                        emit_ptt_start(state, &app);
                    }
                } else {
                    if matches_toggle {
                        state.toggle_armed.store(false, Ordering::SeqCst);
                    }
                    if matches_ptt && state.ptt_start_armed.swap(false, Ordering::SeqCst) {
                        let _ = ShortcutPttStop.emit(&app);
                    }
                }
                return CallbackResult::Drop;
            }

            if matches!(event_type, CGEventType::KeyDown) {
                if matches_toggle {
                    emit_toggle(state, &app);
                }
                if matches_ptt {
                    emit_ptt_start(state, &app);
                }
                return CallbackResult::Drop;
            } else if matches!(event_type, CGEventType::KeyUp) {
                if matches_toggle {
                    state.toggle_armed.store(false, Ordering::SeqCst);
                }
                if matches_ptt && state.ptt_start_armed.swap(false, Ordering::SeqCst) {
                    let _ = ShortcutPttStop.emit(&app);
                }
                return CallbackResult::Drop;
            }

            CallbackResult::Keep
        },
    );

    match tap_result {
        Ok(tap) => {
            info!("Modifier CGEventTap installed");
            store_and_emit_modifier_tap_status(&app_for_status, true);
            tap_port_ptr.store(
                tap.mach_port().as_concrete_TypeRef() as usize,
                Ordering::Relaxed,
            );
            let Ok(loop_source) = tap.mach_port().create_runloop_source(0) else {
                store_and_emit_modifier_tap_status(&app_for_status, false);
                return false;
            };
            let current_loop = core_foundation::runloop::CFRunLoop::get_current();
            current_loop.add_source(&loop_source, unsafe {
                core_foundation::runloop::kCFRunLoopCommonModes
            });
            tap.enable();
            core_foundation::runloop::CFRunLoop::run_current();
            false
        }
        Err(_) => false,
    }
}

fn emit_toggle(state: &AppState, app: &AppHandle) {
    // Key-repeat (and extra FlagsChanged) must not re-fire Toggle.
    if !state.toggle_armed.swap(true, Ordering::SeqCst) {
        let _ = ShortcutToggle.emit(app);
    }
}

fn emit_ptt_start(state: &AppState, app: &AppHandle) {
    if state.ptt_is_paused() {
        state.ptt_start_armed.store(false, Ordering::SeqCst);
        return;
    }
    // Key-repeat (and extra FlagsChanged) must not re-emit Start.
    if !state.ptt_start_armed.swap(true, Ordering::SeqCst) {
        let _ = ShortcutPttStart.emit(app);
    }
}

/// macOS virtual keycodes for modifier-only PTT/Toggle and F5–F12 (SOU-032).
fn shortcut_matches_keycode(shortcut: &str, keycode: i64) -> bool {
    matches!(
        (shortcut, keycode),
        ("Fn", 63)
            | ("MetaLeft", 55)
            | ("MetaRight", 54)
            | ("ShiftLeft", 56)
            | ("ShiftRight", 60)
            | ("AltLeft", 58)
            | ("AltRight", 61)
            | ("ControlLeft", 59)
            | ("ControlRight", 62)
            | ("F5", 96)
            | ("F6", 97)
            | ("F7", 98)
            | ("F8", 100)
            | ("F9", 101)
            | ("F10", 109)
            | ("F11", 103)
            | ("F12", 111)
    )
}

#[cfg(test)]
mod tests {
    use super::{
        ShortcutRegistrationTarget, is_native_shortcut, shortcut_matches_keycode,
        shortcut_registration_target,
    };

    #[test]
    fn modifier_and_fn_keycodes() {
        assert!(shortcut_matches_keycode("Fn", 63));
        assert!(shortcut_matches_keycode("MetaLeft", 55));
        assert!(shortcut_matches_keycode("ControlRight", 62));
        assert!(!shortcut_matches_keycode("Fn", 55));
    }

    #[test]
    fn f_keys_f5_through_f12() {
        for (name, code) in [
            ("F5", 96),
            ("F6", 97),
            ("F7", 98),
            ("F8", 100),
            ("F9", 101),
            ("F10", 109),
            ("F11", 103),
            ("F12", 111),
        ] {
            assert!(shortcut_matches_keycode(name, code), "{name}");
            assert!(is_native_shortcut(name), "{name}");
        }
        assert!(!shortcut_matches_keycode("F5", 122));
        assert!(!is_native_shortcut("F4"));
        assert!(!is_native_shortcut("CommandOrControl+Shift+Space"));
    }

    #[test]
    fn native_toggle_routes_to_tap_not_plugin() {
        assert_eq!(
            shortcut_registration_target("Fn"),
            ShortcutRegistrationTarget::Native
        );
        assert_eq!(
            shortcut_registration_target("MetaRight"),
            ShortcutRegistrationTarget::Native
        );
        assert_eq!(
            shortcut_registration_target("F8"),
            ShortcutRegistrationTarget::Native
        );
        assert_eq!(
            shortcut_registration_target("CommandOrControl+Shift+Space"),
            ShortcutRegistrationTarget::Plugin
        );
        assert_eq!(
            shortcut_registration_target(""),
            ShortcutRegistrationTarget::None
        );
    }
}
