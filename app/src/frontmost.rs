//! Frontmost application name for dictation polish context.
//! Capture at shortcut press, before the pill steals focus.

/// Localized name of the frontmost app, or `None` when unavailable.
pub fn localized_name() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        macos_localized_name()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

fn normalize_localized_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(target_os = "macos")]
fn macos_localized_name() -> Option<String> {
    // AppKit generally wants the main thread. This runs from a Tauri command
    // thread; NSWorkspace.frontmostApplication is often OK off-main for a
    // name read. Return None on failure rather than panicking the command.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let name = objc2_app_kit::NSWorkspace::sharedWorkspace()
            .frontmostApplication()?
            .localizedName()?;
        Some(name.to_string())
    }));
    result
        .ok()
        .flatten()
        .as_deref()
        .and_then(normalize_localized_name)
}

#[cfg(test)]
mod tests {
    #[test]
    fn missing_frontmost_is_none_off_macos() {
        #[cfg(not(target_os = "macos"))]
        assert!(super::localized_name().is_none());
    }

    #[test]
    fn empty_or_whitespace_names_are_none() {
        assert_eq!(super::normalize_localized_name(""), None);
        assert_eq!(super::normalize_localized_name("   "), None);
        assert_eq!(super::normalize_localized_name("\t\n"), None);
        assert_eq!(
            super::normalize_localized_name("Safari"),
            Some("Safari".to_string())
        );
        assert_eq!(
            super::normalize_localized_name("  Mail  "),
            Some("Mail".to_string())
        );
    }
}
