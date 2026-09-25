use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use core_foundation::base::TCFType;
use core_graphics::event::{
    CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult,
};
use tracing::{error, info};

use crate::app_events::ModifierTapStatus;
use crate::native::bridge::{self, NativeAction};
use crate::state::AppState;

/// Last `ModifierTapStatus` observed. Edge-triggered install success/failure;
/// a read, not a rebuild.
static MODIFIER_TAP_STATUS: Mutex<Option<ModifierTapStatus>> = Mutex::new(None);

/// Snapshot for `commands::get_modifier_tap_status`. A read, never a rebuild.
pub fn modifier_tap_status() -> Option<ModifierTapStatus> {
    MODIFIER_TAP_STATUS.lock().ok().and_then(|guard| *guard)
}

fn store_modifier_tap_status(installed: bool) {
    if let Ok(mut guard) = MODIFIER_TAP_STATUS.lock() {
        *guard = Some(ModifierTapStatus { installed });
    }
}

/// Where a dictation shortcut is registered (SOU-032 / SOU-115).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortcutRegistrationTarget {
    /// Empty binding — nothing to register.
    None,
    /// Single native key handled by the CGEventTap.
    Native,
    /// Classic combo handled by `native::shortcuts`.
    Plugin,
}

/// Single-key bindings the combo mechanism cannot register. Combos go
/// through `native::shortcuts` (SOU-032). Shared by Toggle and PTT (SOU-115).
///
/// The settings UI needs the same list to warn that a binding will require
/// Accessibility, so `commands::settings::get_native_shortcuts` hands it over
/// rather than letting the frontend keep a copy.
pub(crate) const NATIVE_SHORTCUTS: [&str; 16] = [
    "MetaLeft",
    "MetaRight",
    "ShiftLeft",
    "ShiftRight",
    "AltLeft",
    "AltRight",
    "ControlLeft",
    "ControlRight",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
];

pub(crate) fn is_native_shortcut(shortcut: &str) -> bool {
    NATIVE_SHORTCUTS.contains(&shortcut)
}

/// Route a stored accelerator to the native tap or the combo mechanism.
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
/// Zero until the tap is installed, and back to zero before the tap is
/// dropped.
static TAP_PORT: AtomicUsize = AtomicUsize::new(0);

/// Serializes `CGEventTapEnable` against the teardown that invalidates the
/// port. `CFRunLoop::run_current` returns when the runloop has no sources
/// left, and dropping the `CGEventTap` invalidates its `CFMachPort`; without
/// this lock a concurrent `sync_modifier_tap` could load the port before the
/// clear and call `CGEventTapEnable` on freed memory.
static TAP_PORT_LOCK: Mutex<()> = Mutex::new(());

/// Whether a configured shortcut currently needs the tap.
static TAP_WANTED: AtomicBool = AtomicBool::new(false);

/// Whether the runloop thread has been spawned. It cannot be unspawned, so
/// an unwanted tap is disabled rather than torn down.
static TAP_THREAD_SPAWNED: AtomicBool = AtomicBool::new(false);

/// Give startup time to finish before the callback reads `AppState`. Status
/// is not reported during this window so the settings banner cannot flash on
/// every launch (SOU-116 risk zone).
const TAP_STARTUP_GRACE: Duration = Duration::from_millis(500);

/// How often the thread re-reads the Accessibility snapshot while it waits
/// for the grant. A read, never a request: it cannot raise a prompt.
const TAP_PERMISSION_POLL: Duration = Duration::from_secs(2);

/// Wakes the waiting thread when `TAP_WANTED` changes. Without it the thread
/// keeps ticking every two seconds for the life of the process even after the
/// user moves both shortcuts to combos and nothing needs the tap.
static TAP_WANTED_CHANGED: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// Block until the tap is wanted again. Returns immediately when it already
/// is; a spurious wake just re-reads `TAP_WANTED`, which is the condition.
fn wait_until_tap_wanted() {
    let (lock, cv) = &TAP_WANTED_CHANGED;
    let mut changed = lock.lock().unwrap_or_else(|e| e.into_inner());
    while !TAP_WANTED.load(Ordering::SeqCst) {
        *changed = false;
        changed = cv
            .wait(changed)
            .unwrap_or_else(|e: std::sync::PoisonError<_>| e.into_inner());
    }
}

fn signal_tap_wanted_changed() {
    let (lock, cv) = &TAP_WANTED_CHANGED;
    if let Ok(mut changed) = lock.lock() {
        *changed = true;
    }
    cv.notify_all();
}

/// Install the native tap, or take it out of the event path, to match the
/// shortcuts the user actually configured. Creating a `CGEventTap` is a TCC
/// request: done with no shortcut needing it, macOS raises an Accessibility
/// prompt nobody asked for, and an active HID tap sits in front of every
/// keystroke on the machine. Called from `register_shortcuts`, so a combo
/// saved in Settings takes the tap out and a single key puts it back without
/// a restart.
pub fn sync_modifier_tap(state: Arc<AppState>, needed: bool) {
    TAP_WANTED.store(needed, Ordering::SeqCst);
    signal_tap_wanted_changed();
    set_tap_enabled(needed);
    if !needed || TAP_THREAD_SPAWNED.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::spawn(move || run_modifier_tap(state));
}

/// True when either configured shortcut is a single native key.
pub(crate) fn tap_is_needed(toggle: &str, push_to_talk: &str) -> bool {
    [toggle, push_to_talk]
        .iter()
        .any(|s| shortcut_registration_target(s) == ShortcutRegistrationTarget::Native)
}

fn run_modifier_tap(state: Arc<AppState>) {
    thread::sleep(TAP_STARTUP_GRACE);
    let mut warned = false;
    loop {
        // Nothing wants the tap: sleep on the condvar instead of ticking
        // forever. `sync_modifier_tap` wakes this up when a single-key
        // shortcut comes back.
        wait_until_tap_wanted();
        // `AXIsProcessTrusted` is a snapshot read, `CGEventTap::new` is a
        // TCC request. Reading first is what keeps the wait from raising a
        // prompt every couple of seconds; the banner carries the news
        // instead (SOU-116).
        if crate::permissions::accessibility_granted() && install_and_run(Arc::clone(&state)) {
            return;
        }
        store_modifier_tap_status(false);
        if !warned {
            error!("Native shortcut tap not installed. Is Accessibility granted?");
            warned = true;
        }
        thread::sleep(TAP_PERMISSION_POLL);
    }
}

#[cfg(target_os = "macos")]
fn set_tap_enabled(enabled: bool) {
    let _guard = TAP_PORT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

fn install_and_run(state: Arc<AppState>) -> bool {
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

            let toggle_shortcut = {
                // A poisoned lock must not panic here: this runs inside the
                // C callback of an active HID tap, in front of every
                // keystroke on the machine.
                let lock = state
                    .modifier_toggle_shortcut
                    .read()
                    .unwrap_or_else(|e| e.into_inner());
                lock.clone()
            };
            let ptt_shortcut = {
                let lock = state
                    .modifier_ptt_shortcut
                    .read()
                    .unwrap_or_else(|e| e.into_inner());
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
                    _ => false,
                };

                if is_pressed {
                    if matches_toggle {
                        dispatch_toggle(&state);
                    }
                    if matches_ptt {
                        dispatch_ptt_start(&state);
                    }
                } else {
                    if matches_toggle {
                        state.toggle_armed.store(false, Ordering::SeqCst);
                    }
                    if matches_ptt && state.ptt_start_armed.swap(false, Ordering::SeqCst) {
                        bridge::dispatch(NativeAction::PttStop);
                    }
                }
                return CallbackResult::Drop;
            }

            if matches!(event_type, CGEventType::KeyDown) {
                if matches_toggle {
                    dispatch_toggle(&state);
                }
                if matches_ptt {
                    dispatch_ptt_start(&state);
                }
                return CallbackResult::Drop;
            } else if matches!(event_type, CGEventType::KeyUp) {
                if matches_toggle {
                    state.toggle_armed.store(false, Ordering::SeqCst);
                }
                if matches_ptt && state.ptt_start_armed.swap(false, Ordering::SeqCst) {
                    bridge::dispatch(NativeAction::PttStop);
                }
                return CallbackResult::Drop;
            }

            CallbackResult::Keep
        },
    );

    match tap_result {
        Ok(tap) => {
            info!("Modifier CGEventTap installed");
            store_modifier_tap_status(true);
            TAP_PORT.store(
                tap.mach_port().as_concrete_TypeRef() as usize,
                Ordering::SeqCst,
            );
            let Ok(loop_source) = tap.mach_port().create_runloop_source(0) else {
                clear_tap_port();
                store_modifier_tap_status(false);
                return false;
            };
            let current_loop = core_foundation::runloop::CFRunLoop::get_current();
            current_loop.add_source(&loop_source, unsafe {
                core_foundation::runloop::kCFRunLoopCommonModes
            });
            // A shortcut saved while the tap was being installed decides here.
            set_tap_enabled(TAP_WANTED.load(Ordering::SeqCst));
            core_foundation::runloop::CFRunLoop::run_current();
            // The runloop gave up (no sources left). Retire the port before
            // `tap` drops and invalidates it, so no enable can reach it.
            clear_tap_port();
            drop(tap);
            false
        }
        Err(_) => false,
    }
}

/// Zero `TAP_PORT` under the lock `set_tap_enabled` takes, so a concurrent
/// enable either ran before the clear (on a still-valid port) or sees zero.
fn clear_tap_port() {
    let _guard = TAP_PORT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    TAP_PORT.store(0, Ordering::SeqCst);
}

fn dispatch_toggle(state: &AppState) {
    // Key-repeat (and extra FlagsChanged) must not re-fire Toggle.
    if !state.toggle_armed.swap(true, Ordering::SeqCst) {
        bridge::dispatch(NativeAction::ToggleDictation);
    }
}

fn dispatch_ptt_start(state: &AppState) {
    if state.ptt_is_paused() {
        state.ptt_start_armed.store(false, Ordering::SeqCst);
        return;
    }
    // Key-repeat (and extra FlagsChanged) must not re-fire PttStart.
    if !state.ptt_start_armed.swap(true, Ordering::SeqCst) {
        bridge::dispatch(NativeAction::PttStart);
    }
}

/// macOS virtual keycodes for modifier-only PTT/Toggle and F5–F12 (SOU-032).
fn shortcut_matches_keycode(shortcut: &str, keycode: i64) -> bool {
    matches!(
        (shortcut, keycode),
        ("MetaLeft", 55)
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
        NATIVE_SHORTCUTS, ShortcutRegistrationTarget, is_native_shortcut, shortcut_matches_keycode,
        shortcut_registration_target, tap_is_needed,
    };

    /// AC5: the sixteen values are what users already have bound. The list
    /// is now the only declaration, and the frontend reads it over IPC, so a
    /// change here silently changes existing bindings on both sides.
    #[test]
    fn native_shortcut_list_is_unchanged() {
        assert_eq!(
            NATIVE_SHORTCUTS,
            [
                "MetaLeft",
                "MetaRight",
                "ShiftLeft",
                "ShiftRight",
                "AltLeft",
                "AltRight",
                "ControlLeft",
                "ControlRight",
                "F5",
                "F6",
                "F7",
                "F8",
                "F9",
                "F10",
                "F11",
                "F12",
            ]
        );
    }

    #[test]
    fn every_listed_shortcut_routes_to_the_tap() {
        for shortcut in NATIVE_SHORTCUTS {
            assert!(is_native_shortcut(shortcut), "{shortcut}");
            assert_eq!(
                shortcut_registration_target(shortcut),
                ShortcutRegistrationTarget::Native,
                "{shortcut}"
            );
        }
    }

    #[test]
    fn modifier_keycodes() {
        assert!(shortcut_matches_keycode("ShiftLeft", 56));
        assert!(shortcut_matches_keycode("MetaLeft", 55));
        assert!(shortcut_matches_keycode("ControlRight", 62));
        assert!(!shortcut_matches_keycode("ShiftLeft", 55));
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
            shortcut_registration_target("ShiftLeft"),
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
        assert!(tap_is_needed("ShiftLeft", "CommandOrControl+Shift+D"));
        assert!(tap_is_needed(
            "CommandOrControl+Shift+Space",
            "ControlRight"
        ));
        assert!(tap_is_needed("F8", "ShiftLeft"));
    }
}
