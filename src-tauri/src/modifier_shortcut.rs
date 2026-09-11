use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
    let changed = match MODIFIER_TAP_STATUS.lock() {
        Ok(mut guard) => {
            let changed = guard.map(|s| s.installed) != Some(installed);
            *guard = Some(status);
            changed
        }
        Err(_) => true,
    };
    // The waiting thread re-reads the permission every couple of seconds.
    // The banner only needs the transitions, not the polling.
    if changed {
        let _ = status.emit(app);
    }
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

/// Mach port of the installed tap, for enable/disable from any thread.
/// Zero until the tap is installed; the runloop thread never lets it go.
static TAP_PORT: AtomicUsize = AtomicUsize::new(0);

/// Whether a configured shortcut currently needs the tap.
static TAP_WANTED: AtomicBool = AtomicBool::new(false);

/// Whether the runloop thread has been spawned. It cannot be unspawned, so
/// an unwanted tap is disabled rather than torn down.
static TAP_THREAD_SPAWNED: AtomicBool = AtomicBool::new(false);

/// Give the Tauri setup hook time to finish managing `AppState` before the
/// callback reads it. Status is not emitted during this window so the
/// settings banner cannot flash on every launch (SOU-116 risk zone).
const TAP_STARTUP_GRACE: Duration = Duration::from_millis(500);

/// How often the thread re-reads the Accessibility snapshot while it waits
/// for the grant. A read, never a request: it cannot raise a prompt.
const TAP_PERMISSION_POLL: Duration = Duration::from_secs(2);

/// Install the native tap, or take it out of the event path, to match the
/// shortcuts the user actually configured. Creating a `CGEventTap` is a TCC
/// request: done with no shortcut needing it, macOS raises an Accessibility
/// prompt nobody asked for, and an active HID tap sits in front of every
/// keystroke on the machine. Called from `register_shortcuts`, so a combo
/// saved in Settings takes the tap out and a single key puts it back without
/// a restart.
pub fn sync_modifier_tap(app: &AppHandle, needed: bool) {
    TAP_WANTED.store(needed, Ordering::SeqCst);
    set_tap_enabled(needed);
    if !needed || TAP_THREAD_SPAWNED.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    thread::spawn(move || run_modifier_tap(app));
}

/// True when either configured shortcut is a single native key.
pub(crate) fn tap_is_needed(toggle: &str, push_to_talk: &str) -> bool {
    [toggle, push_to_talk]
        .iter()
        .any(|s| shortcut_registration_target(s) == ShortcutRegistrationTarget::Native)
}

fn run_modifier_tap(app: AppHandle) {
    thread::sleep(TAP_STARTUP_GRACE);
    let mut warned = false;
    loop {
        if TAP_WANTED.load(Ordering::SeqCst) {
            // `AXIsProcessTrusted` is a snapshot read, `CGEventTap::new` is a
            // TCC request. Reading first is what keeps the wait from raising a
            // prompt every couple of seconds; the banner carries the news
            // instead (SOU-116).
            if crate::permissions::accessibility_granted() && install_and_run(app.clone()) {
                return;
            }
            store_and_emit_modifier_tap_status(&app, false);
            if !warned {
                error!("Native shortcut tap not installed. Is Accessibility granted?");
                warned = true;
            }
        }
        thread::sleep(TAP_PERMISSION_POLL);
    }
}

#[cfg(target_os = "macos")]
fn set_tap_enabled(enabled: bool) {
    let ptr = TAP_PORT.load(Ordering::SeqCst);
    if ptr == 0 {
        return;
    }
    unsafe {
        // CGEventTapEnable takes a CFMachPortRef.
        unsafe extern "C" {
            fn CGEventTapEnable(tap: *const std::ffi::c_void, enable: bool);
        }
        CGEventTapEnable(ptr as *const _, enabled);
    }
}

#[cfg(not(target_os = "macos"))]
fn set_tap_enabled(_enabled: bool) {}

fn install_and_run(app: AppHandle) -> bool {
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
                set_tap_enabled(TAP_WANTED.load(Ordering::SeqCst));
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
            TAP_PORT.store(
                tap.mach_port().as_concrete_TypeRef() as usize,
                Ordering::SeqCst,
            );
            let Ok(loop_source) = tap.mach_port().create_runloop_source(0) else {
                TAP_PORT.store(0, Ordering::SeqCst);
                store_and_emit_modifier_tap_status(&app_for_status, false);
                return false;
            };
            let current_loop = core_foundation::runloop::CFRunLoop::get_current();
            current_loop.add_source(&loop_source, unsafe {
                core_foundation::runloop::kCFRunLoopCommonModes
            });
            // A shortcut saved while the tap was being installed decides here.
            set_tap_enabled(TAP_WANTED.load(Ordering::SeqCst));
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
        shortcut_registration_target, tap_is_needed,
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

    /// The tap is an Accessibility request and sits in front of every
    /// keystroke on the machine. Default install: a combo and an empty PTT,
    /// so no tap and no permission prompt at startup.
    #[test]
    fn tap_is_only_needed_for_a_single_key_binding() {
        assert!(!tap_is_needed(
            "CommandOrControl+Shift+Space",
            "CommandOrControl+Shift+D"
        ));
        assert!(!tap_is_needed("", ""));
        assert!(tap_is_needed("Fn", "CommandOrControl+Shift+D"));
        assert!(tap_is_needed(
            "CommandOrControl+Shift+Space",
            "ControlRight"
        ));
        assert!(tap_is_needed("F8", "Fn"));
    }
}
