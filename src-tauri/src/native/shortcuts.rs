//! Native replacement for `tauri-plugin-global-shortcut` (SOU-191).
//!
//! `global-hotkey` is the same crate the Tauri plugin wraps internally, used
//! directly here instead of through Tauri's plugin API. Accelerator strings
//! (`"CommandOrControl+Shift+Space"`) parse identically either way — the
//! plugin's `Shortcut` is itself a thin wrapper over `global_hotkey::hotkey::HotKey`
//! — so `ShortcutSettings`'s stored strings need no migration.
//!
//! Single-key bindings (a lone modifier or an F-key) never reach this module:
//! `modifier_shortcut.rs`'s `CGEventTap` handles those, unchanged by this
//! ticket except for how it reports back (see `native::bridge`).

use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tracing::{info, warn};

use std::sync::Arc;

use crate::modifier_shortcut::{ShortcutRegistrationTarget, shortcut_registration_target};
use crate::native::bridge::{self, NativeAction};
use crate::settings::ShortcutSettings;
use crate::state::AppState;

const ESCAPE: &str = "Escape";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Toggle,
    Ptt,
    EscapeCancel,
}

fn manager() -> &'static GlobalHotKeyManager {
    static MANAGER: OnceLock<GlobalHotKeyManager> = OnceLock::new();
    MANAGER.get_or_init(|| {
        GlobalHotKeyManager::new().expect("failed to initialize the global hotkey manager")
    })
}

/// Currently-registered combo shortcuts, keyed by `HotKey::id()`. Anything
/// registered through the native `CGEventTap` (single keys) never appears
/// here — that registry lives in `modifier_shortcut.rs`.
static REGISTRY: Mutex<Vec<(HotKey, Role)>> = Mutex::new(Vec::new());

fn unregister_all() -> Result<(), String> {
    let mut guard = REGISTRY.lock().map_err(|_| "shortcut registry poisoned")?;
    if guard.is_empty() {
        return Ok(());
    }
    let hotkeys: Vec<HotKey> = guard.iter().map(|(hk, _)| *hk).collect();
    manager()
        .unregister_all(&hotkeys)
        .map_err(|e| format!("Unregister: {e}"))?;
    guard.clear();
    Ok(())
}

fn is_registered(accelerator: &str) -> bool {
    let Ok(hk) = HotKey::from_str(accelerator) else {
        return false;
    };
    REGISTRY
        .lock()
        .map(|guard| guard.iter().any(|(existing, _)| existing.id() == hk.id()))
        .unwrap_or(false)
}

fn unregister_one(accelerator: &str) -> Result<(), String> {
    let Ok(hk) = HotKey::from_str(accelerator) else {
        return Ok(());
    };
    let mut guard = REGISTRY.lock().map_err(|_| "shortcut registry poisoned")?;
    let Some(pos) = guard
        .iter()
        .position(|(existing, _)| existing.id() == hk.id())
    else {
        return Ok(());
    };
    manager()
        .unregister(hk)
        .map_err(|e| format!("Unregister '{accelerator}': {e}"))?;
    guard.remove(pos);
    Ok(())
}

fn register_role(accelerator: &str, role: Role) -> Result<(), String> {
    let hk = HotKey::from_str(accelerator)
        .map_err(|e| format!("Invalid accelerator '{accelerator}': {e}"))?;
    manager()
        .register(hk)
        .map_err(|e| format!("Register shortcut '{accelerator}': {e}"))?;
    let mut guard = REGISTRY.lock().map_err(|_| "shortcut registry poisoned")?;
    guard.push((hk, role));
    Ok(())
}

/// Register the toggle/push-to-talk combo shortcuts. Native single-key
/// shortcuts and the Escape dictation-cancel binding are handled separately
/// (`modifier_shortcut::sync_modifier_tap`, `dictation_cancel::sync`) exactly
/// as before this ticket, just no longer through a Tauri plugin.
pub fn register_shortcuts(
    state: &Arc<AppState>,
    shortcuts: &ShortcutSettings,
) -> Result<(), String> {
    unregister_all()?;
    crate::dictation_cancel::mark_unregistered();

    let toggle_target = shortcut_registration_target(&shortcuts.toggle);
    let ptt_target = shortcut_registration_target(&shortcuts.push_to_talk);

    {
        let mut lock = state.modifier_toggle_shortcut.write().unwrap();
        *lock = match toggle_target {
            ShortcutRegistrationTarget::Native => Some(shortcuts.toggle.clone()),
            ShortcutRegistrationTarget::None | ShortcutRegistrationTarget::Plugin => None,
        };
    }
    {
        let mut lock = state.modifier_ptt_shortcut.write().unwrap();
        *lock = match ptt_target {
            ShortcutRegistrationTarget::Native => Some(shortcuts.push_to_talk.clone()),
            ShortcutRegistrationTarget::None | ShortcutRegistrationTarget::Plugin => None,
        };
    }
    state
        .toggle_armed
        .store(false, std::sync::atomic::Ordering::SeqCst);

    crate::modifier_shortcut::sync_modifier_tap(
        Arc::clone(state),
        crate::modifier_shortcut::tap_is_needed(&shortcuts.toggle, &shortcuts.push_to_talk),
    );

    if toggle_target == ShortcutRegistrationTarget::Plugin {
        register_role(&shortcuts.toggle, Role::Toggle)
            .map_err(|e| format!("Register toggle shortcut '{}': {e}", shortcuts.toggle))?;
        info!(shortcut = shortcuts.toggle, "Toggle shortcut registered");
    } else if toggle_target == ShortcutRegistrationTarget::Native {
        info!(
            shortcut = shortcuts.toggle,
            "Toggle shortcut registered via native tap"
        );
    }

    if ptt_target == ShortcutRegistrationTarget::Plugin {
        register_role(&shortcuts.push_to_talk, Role::Ptt)
            .map_err(|e| format!("Register PTT shortcut '{}': {e}", shortcuts.push_to_talk))?;
        info!(
            shortcut = shortcuts.push_to_talk,
            "Push-to-talk shortcut registered"
        );
    } else if ptt_target == ShortcutRegistrationTarget::Native {
        info!(
            shortcut = shortcuts.push_to_talk,
            "Push-to-talk shortcut registered via native tap"
        );
    }

    if let Ok(machine) = state.current_machine_state() {
        crate::dictation_cancel::sync(state, &machine);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Escape dictation-cancel binding (ported from dictation_cancel.rs's use of
// `app.global_shortcut()`).
// ---------------------------------------------------------------------------

pub fn escape_is_armed() -> bool {
    is_registered(ESCAPE)
}

pub fn arm_escape_cancel() -> Result<(), String> {
    unregister_one(ESCAPE)?;
    register_role(ESCAPE, Role::EscapeCancel)
}

pub fn disarm_escape() -> Result<(), String> {
    unregister_one(ESCAPE)
}

/// Re-bind Escape as the user's actual toggle/PTT shortcut after the cancel
/// binding is torn down, if that is what they configured (mirrors
/// `dictation_cancel::restore_user_escape`).
pub fn restore_escape_role(shortcuts: &ShortcutSettings) -> Result<(), String> {
    if shortcuts.toggle == ESCAPE {
        return register_role(ESCAPE, Role::Toggle);
    }
    if shortcuts.push_to_talk == ESCAPE
        && !crate::modifier_shortcut::is_native_shortcut(&shortcuts.push_to_talk)
    {
        return register_role(ESCAPE, Role::Ptt);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

fn role_for(id: u32) -> Option<Role> {
    REGISTRY
        .lock()
        .ok()
        .and_then(|guard| guard.iter().find(|(hk, _)| hk.id() == id).map(|(_, r)| *r))
}

/// Blocks on the `global-hotkey` event channel and dispatches. Must run on a
/// dedicated background thread — `GlobalHotKeyEvent::receiver()` is a plain
/// `crossbeam_channel::Receiver`, registration itself happens wherever
/// `register_shortcuts`/`arm_escape_cancel` are called from (the Slint main
/// thread, same requirement `dictation_cancel.rs` already documented for the
/// Tauri plugin — a CFRunLoop must be alive on the registering thread).
pub fn spawn_event_loop(state: std::sync::Arc<AppState>) {
    std::thread::Builder::new()
        .name("global-hotkey-events".into())
        .spawn(move || {
            let receiver = GlobalHotKeyEvent::receiver();
            for event in receiver {
                if event.state != HotKeyState::Pressed && event.state != HotKeyState::Released {
                    continue;
                }
                let Some(role) = role_for(event.id) else {
                    continue;
                };
                match (role, event.state) {
                    (Role::Toggle, HotKeyState::Pressed) => {
                        if !state
                            .toggle_armed
                            .swap(true, std::sync::atomic::Ordering::SeqCst)
                        {
                            bridge::dispatch(NativeAction::ToggleDictation);
                        }
                    }
                    (Role::Toggle, HotKeyState::Released) => {
                        state
                            .toggle_armed
                            .store(false, std::sync::atomic::Ordering::SeqCst);
                    }
                    (Role::Ptt, HotKeyState::Pressed) => {
                        if state.ptt_is_paused() {
                            state
                                .ptt_start_armed
                                .store(false, std::sync::atomic::Ordering::SeqCst);
                        } else if !state
                            .ptt_start_armed
                            .swap(true, std::sync::atomic::Ordering::SeqCst)
                        {
                            bridge::dispatch(NativeAction::PttStart);
                        }
                    }
                    (Role::Ptt, HotKeyState::Released) => {
                        if state
                            .ptt_start_armed
                            .swap(false, std::sync::atomic::Ordering::SeqCst)
                        {
                            bridge::dispatch(NativeAction::PttStop);
                        }
                    }
                    (Role::EscapeCancel, HotKeyState::Pressed) => {
                        let Ok(machine) = state.current_machine_state() else {
                            continue;
                        };
                        if crate::dictation_cancel::should_arm(true, &machine) {
                            bridge::dispatch(NativeAction::CancelDictation);
                        }
                    }
                    (Role::EscapeCancel, HotKeyState::Released) => {}
                }
            }
            warn!("global-hotkey event channel closed; shortcut events will no longer fire");
        })
        .expect("failed to spawn global-hotkey event loop thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_accelerator_syntax_parses_as_a_hotkey() {
        assert!(HotKey::from_str("CommandOrControl+Shift+Space").is_ok());
        assert!(HotKey::from_str("CommandOrControl+Shift+D").is_ok());
    }

    #[test]
    fn escape_parses_as_a_hotkey() {
        assert!(HotKey::from_str(ESCAPE).is_ok());
    }
}
