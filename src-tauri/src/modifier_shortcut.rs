use std::sync::Arc;
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

use crate::app_events::{ShortcutPttStart, ShortcutPttStop};
use crate::state::AppState;

/// Single-key PTT bindings that the plugin cannot register. Combos stay on
/// `tauri-plugin-global-shortcut` (SOU-032).
pub(crate) fn is_native_ptt_shortcut(shortcut: &str) -> bool {
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

pub fn start_modifier_tap(app: AppHandle) {
    thread::spawn(move || {
        // Wait a little before starting to ensure AppState is registered.
        thread::sleep(Duration::from_millis(500));
        let mut warned = false;
        loop {
            if install_and_run(app.clone()) {
                return;
            }
            if !warned {
                error!(
                    "Failed to install modifier CGEventTap. Is Input Monitoring granted? Retrying."
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
            let current_shortcut = {
                let lock = state.modifier_ptt_shortcut.read().unwrap();
                lock.clone()
            };

            let Some(shortcut) = current_shortcut else {
                return CallbackResult::Keep;
            };

            let keycode = event
                .get_integer_value_field(core_graphics::event::EventField::KEYBOARD_EVENT_KEYCODE);
            let flags = event.get_flags();

            if !shortcut_matches_keycode(&shortcut, keycode) {
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
                    emit_ptt_start(state, &app);
                } else if state.ptt_start_armed.swap(false, Ordering::SeqCst) {
                    let _ = ShortcutPttStop.emit(&app);
                }
                return CallbackResult::Drop;
            }

            if matches!(event_type, CGEventType::KeyDown) {
                emit_ptt_start(state, &app);
                return CallbackResult::Drop;
            } else if matches!(event_type, CGEventType::KeyUp) {
                if state.ptt_start_armed.swap(false, Ordering::SeqCst) {
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
            tap_port_ptr.store(
                tap.mach_port().as_concrete_TypeRef() as usize,
                Ordering::Relaxed,
            );
            let Ok(loop_source) = tap.mach_port().create_runloop_source(0) else {
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

/// macOS virtual keycodes for modifier-only PTT and F5–F12 (SOU-032).
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
    use super::{is_native_ptt_shortcut, shortcut_matches_keycode};

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
            assert!(is_native_ptt_shortcut(name), "{name}");
        }
        assert!(!shortcut_matches_keycode("F5", 122));
        assert!(!is_native_ptt_shortcut("F4"));
        assert!(!is_native_ptt_shortcut("CommandOrControl+Shift+Space"));
    }
}
