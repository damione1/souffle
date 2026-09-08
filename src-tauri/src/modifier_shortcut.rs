use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
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

pub fn start_modifier_tap(app: AppHandle) {
    thread::spawn(move || {
        // Wait a little before starting to ensure AppState is registered
        std::thread::sleep(Duration::from_millis(500));
        let app_clone = app.clone();
        
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
                if matches!(event_type, CGEventType::TapDisabledByTimeout) {
                    let ptr = tap_port_clone.load(Ordering::Relaxed);
                    if ptr != 0 {
                        unsafe {
                            // CGEventTapEnable takes a CFMachPortRef.
                            // We use the pointer we stashed.
                            unsafe extern "C" {
                                fn CGEventTapEnable(tap: *const std::ffi::c_void, enable: bool);
                            }
                            CGEventTapEnable(ptr as *const _, true);
                        }
                    }
                    return CallbackResult::Keep;
                }
                if matches!(event_type, CGEventType::TapDisabledByUserInput) {
                    return CallbackResult::Keep;
                }

                let state = match app_clone.try_state::<AppState>() {
                    Some(s) => s.inner().clone(),
                    None => return CallbackResult::Keep,
                };
                let current_shortcut = {
                    let lock = state.modifier_ptt_shortcut.read().unwrap();
                    lock.clone()
                };

                let Some(shortcut) = current_shortcut else {
                    return CallbackResult::Keep;
                };

                let keycode = event.get_integer_value_field(core_graphics::event::EventField::KEYBOARD_EVENT_KEYCODE);
                let flags = event.get_flags();

                let is_match = match (shortcut.as_str(), keycode as i64) {
                    ("Fn", 63) => true,
                    ("MetaLeft", 55) => true,
                    ("MetaRight", 54) => true,
                    ("ShiftLeft", 56) => true,
                    ("ShiftRight", 60) => true,
                    ("AltLeft", 58) => true,
                    ("AltRight", 61) => true,
                    ("ControlLeft", 59) => true,
                    ("ControlRight", 62) => true,
                    _ => false,
                };

                if !is_match {
                    return CallbackResult::Keep;
                }

                if matches!(event_type, CGEventType::FlagsChanged) {
                    let is_pressed = match keycode as i64 {
                        55 | 54 => flags.contains(CGEventFlags::CGEventFlagCommand),
                        56 | 60 => flags.contains(CGEventFlags::CGEventFlagShift),
                        58 | 61 => flags.contains(CGEventFlags::CGEventFlagAlternate),
                        59 | 62 => flags.contains(CGEventFlags::CGEventFlagControl),
                        63 => flags.contains(CGEventFlags::CGEventFlagSecondaryFn),
                        _ => false,
                    };

                    if is_pressed {
                        if state.ptt_is_paused() {
                            state.ptt_start_armed.store(false, Ordering::SeqCst);
                        } else {
                            state.ptt_start_armed.store(true, Ordering::SeqCst);
                            let _ = ShortcutPttStart.emit(&app_clone);
                        }
                    } else {
                        if state.ptt_start_armed.swap(false, Ordering::SeqCst) {
                            let _ = ShortcutPttStop.emit(&app_clone);
                        }
                    }
                    return CallbackResult::Drop;
                }

                if matches!(event_type, CGEventType::KeyDown) {
                    if state.ptt_is_paused() {
                        state.ptt_start_armed.store(false, Ordering::SeqCst);
                    } else {
                        state.ptt_start_armed.store(true, Ordering::SeqCst);
                        let _ = ShortcutPttStart.emit(&app_clone);
                    }
                    return CallbackResult::Drop;
                } else if matches!(event_type, CGEventType::KeyUp) {
                    if state.ptt_start_armed.swap(false, Ordering::SeqCst) {
                        let _ = ShortcutPttStop.emit(&app_clone);
                    }
                    return CallbackResult::Drop;
                }

                CallbackResult::Keep
            },
        );

        match tap_result {
            Ok(tap) => {
                info!("Modifier CGEventTap installed");
                tap_port_ptr.store(tap.mach_port().as_concrete_TypeRef() as usize, Ordering::Relaxed);
                let loop_source = tap.mach_port().create_runloop_source(0).unwrap();
                let current_loop = core_foundation::runloop::CFRunLoop::get_current();
                current_loop.add_source(&loop_source, unsafe { core_foundation::runloop::kCFRunLoopCommonModes });
                tap.enable();
                core_foundation::runloop::CFRunLoop::run_current();
            }
            Err(e) => {
                error!("Failed to install modifier CGEventTap. Is Input Monitoring granted? {e:?}");
            }
        }
    });
}
