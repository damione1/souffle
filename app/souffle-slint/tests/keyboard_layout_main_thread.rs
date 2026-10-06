//! Smoke test for the keyboard-layout lookup behind the shortcut labels
//! (#360). Text Input Sources abort off the main thread and libtest runs
//! every test on a worker thread, so this file has no harness: `main` is the
//! process's main thread, the only one the lookup may run on.
//!
//! The layout of the machine running the tests is unknown (US, AZERTY, a CI
//! runner with no session), so this only checks that the FFI does not crash
//! and that whatever it returns is a usable label.

// The module's own `#[cfg(test)]` unit tests compile here too but only run
// under libtest (`cargo test --bin souffle-slint`), hence the allows.
#[allow(dead_code, unused_imports)]
#[path = "../src/keyboard_layout.rs"]
mod keyboard_layout;

use objc2::MainThreadMarker;

fn main() {
    let mtm = MainThreadMarker::new().expect("harness = false runs main on the main thread");
    let tokens = [
        "A",
        "Q",
        "Z",
        "M",
        "1",
        "0",
        "Minus",
        "Equal",
        "Backquote",
        "Slash",
        "Comma",
    ];
    for token in tokens {
        if let Some(label) = keyboard_layout::key_label(mtm, token) {
            assert!(
                !label.is_empty() && !label.chars().any(char::is_control),
                "{token} -> {label:?}"
            );
        }
    }
    for named in ["Space", "F5", "ArrowUp"] {
        assert_eq!(keyboard_layout::key_label(mtm, named), None, "{named}");
    }
    println!("keyboard layout lookup: ok on the main thread");
}
