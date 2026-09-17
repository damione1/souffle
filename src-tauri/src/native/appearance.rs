//! Resolves the "system" theme setting against the real macOS appearance,
//! the native replacement for the Tauri/browser side's
//! `window.matchMedia("(prefers-color-scheme: dark)")` check.
//!
//! One-shot query, not a live subscription: this is read once at launch (and
//! whenever the user re-opens the theme picker) rather than tracked via a
//! `NSApplication` appearance-changed notification. If macOS's own
//! appearance flips while Soufflé is running and the user has "system"
//! selected, the app picks up the new appearance the next time it restarts
//! or the user touches the theme setting - matching the scope of this pass
//! (SOU-195's theme toggle), not a live OS-appearance listener.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication};
use objc2_foundation::NSArray;

/// `true` if the effective system appearance is Dark Aqua (or a variant of
/// it, e.g. an accessibility high-contrast dark mode - `bestMatchFrom
/// AppearancesWithNames:` is Apple's own recommended way to resolve that,
/// rather than a raw string compare against the base name).
pub fn is_system_dark() -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        // Matches this module's "best-effort, main-thread-only" scope
        // (see `native/dialog.rs`'s identical caveat) - default to dark,
        // this app's own default theme, rather than guessing light.
        return true;
    };
    let app = NSApplication::sharedApplication(mtm);
    let appearance = app.effectiveAppearance();
    // SAFETY: `NSAppearanceNameAqua`/`NSAppearanceNameDarkAqua` are AppKit's
    // own static `NSAppearanceName` constants, read-only for the process
    // lifetime - no aliasing/mutation risk, just an FFI extern static the
    // Rust type system can't vouch for on its own.
    let (aqua, dark_aqua) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
    let candidates = NSArray::from_slice(&[aqua, dark_aqua]);
    let best_match = appearance.bestMatchFromAppearancesWithNames(&candidates);
    best_match.is_some_and(|name| unsafe { &*name == NSAppearanceNameDarkAqua })
}
