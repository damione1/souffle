//! Accessibility helpers for the focused text field.
//! Used by paste-into-selection and learn-from-edit.
//!
//! AXUIElement calls are documented to run on the main thread. Tauri commands
//! may arrive on a worker; we invoke AX anyway and treat failure as
//! `None`/`false`. Do **not** `dispatch_sync` onto the main thread from here —
//! if the main thread is waiting on this command, that deadlocks.

/// Currently selected text in the focused UI element, if any.
pub fn selected_text() -> Result<Option<String>, String> {
    #[cfg(target_os = "macos")]
    {
        macos_selected_text()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(None)
    }
}

/// Entire value of the focused UI element, if readable.
pub fn focused_text() -> Result<Option<String>, String> {
    #[cfg(target_os = "macos")]
    {
        macos_focused_text()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(None)
    }
}

/// Replace the current selection (or insert at caret). Returns `true` if AX
/// applied the text, `false` if the caller should fall back to ⌘V / typing.
pub fn set_selected_text(text: &str) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        macos_set_selected_text(text)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = text;
        Ok(false)
    }
}

/// Empty AX strings are treated as "nothing selected / nothing to report".
fn nonempty_ax_string(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(target_os = "macos")]
fn macos_selected_text() -> Result<Option<String>, String> {
    if !crate::permissions::accessibility_granted() {
        return Ok(None);
    }
    Ok(focused_element().and_then(|el| {
        copy_string_attribute(&el, accessibility_sys::kAXSelectedTextAttribute)
            .and_then(nonempty_ax_string)
    }))
}

#[cfg(target_os = "macos")]
fn macos_focused_text() -> Result<Option<String>, String> {
    if !crate::permissions::accessibility_granted() {
        return Ok(None);
    }
    Ok(focused_element()
        .and_then(|el| copy_string_attribute(&el, accessibility_sys::kAXValueAttribute)))
}

#[cfg(target_os = "macos")]
fn macos_set_selected_text(text: &str) -> Result<bool, String> {
    if !crate::permissions::accessibility_granted() {
        return Err(crate::clipboard::ACCESSIBILITY_STALE_ERROR.to_string());
    }
    let Some(element) = focused_element() else {
        return Ok(false);
    };
    // Only AXSelectedText — setting AXValue replaces the whole field.
    Ok(set_string_attribute(
        &element,
        accessibility_sys::kAXSelectedTextAttribute,
        text,
    ))
}

/// System-wide element → `kAXFocusedUIElementAttribute`.
///
/// Create/Copy AXUIElement and CFTypeRef values are wrapped in `CFRetained`
/// so they are released on drop (no leaks).
#[cfg(target_os = "macos")]
fn focused_element() -> Option<objc2_core_foundation::CFRetained<objc2_core_foundation::CFType>> {
    use accessibility_sys::{kAXFocusedUIElementAttribute, AXUIElementCreateSystemWide};

    let system_raw = unsafe { AXUIElementCreateSystemWide() };
    let system = retain_created(system_raw)?;
    copy_attribute(&system, kAXFocusedUIElementAttribute)
}

#[cfg(target_os = "macos")]
fn retain_created(
    raw: accessibility_sys::AXUIElementRef,
) -> Option<objc2_core_foundation::CFRetained<objc2_core_foundation::CFType>> {
    use objc2_core_foundation::{CFRetained, CFType};
    use std::ptr::NonNull;

    let ptr = NonNull::new(raw.cast::<CFType>())?;
    // AXUIElementCreate* follows the Create rule (+1 retain).
    Some(unsafe { CFRetained::from_raw(ptr) })
}

#[cfg(target_os = "macos")]
fn as_ax(
    element: &objc2_core_foundation::CFRetained<objc2_core_foundation::CFType>,
) -> accessibility_sys::AXUIElementRef {
    use objc2_core_foundation::CFRetained;
    CFRetained::as_ptr(element).as_ptr().cast()
}

#[cfg(target_os = "macos")]
fn copy_attribute(
    element: &objc2_core_foundation::CFRetained<objc2_core_foundation::CFType>,
    attribute: &'static str,
) -> Option<objc2_core_foundation::CFRetained<objc2_core_foundation::CFType>> {
    use accessibility_sys::{kAXErrorSuccess, AXUIElementCopyAttributeValue};
    use objc2_core_foundation::{CFRetained, CFString, CFType};
    use std::ptr::NonNull;

    let name = CFString::from_static_str(attribute);
    let mut value = std::ptr::null();
    let err = unsafe {
        AXUIElementCopyAttributeValue(
            as_ax(element),
            CFRetained::as_ptr(&name).as_ptr().cast(),
            &mut value,
        )
    };
    if err != kAXErrorSuccess {
        return None;
    }
    let ptr = NonNull::new(value.cast_mut().cast::<CFType>())?;
    // CopyAttributeValue follows the Copy rule (+1 retain).
    Some(unsafe { CFRetained::from_raw(ptr) })
}

#[cfg(target_os = "macos")]
fn copy_string_attribute(
    element: &objc2_core_foundation::CFRetained<objc2_core_foundation::CFType>,
    attribute: &'static str,
) -> Option<String> {
    use objc2_core_foundation::CFString;

    let value = copy_attribute(element, attribute)?;
    value.downcast_ref::<CFString>().map(ToString::to_string)
}

#[cfg(target_os = "macos")]
fn set_string_attribute(
    element: &objc2_core_foundation::CFRetained<objc2_core_foundation::CFType>,
    attribute: &'static str,
    text: &str,
) -> bool {
    use accessibility_sys::{kAXErrorSuccess, AXUIElementSetAttributeValue};
    use objc2_core_foundation::{CFRetained, CFString};

    let name = CFString::from_static_str(attribute);
    let value = CFString::from_str(text);
    let err = unsafe {
        AXUIElementSetAttributeValue(
            as_ax(element),
            CFRetained::as_ptr(&name).as_ptr().cast(),
            CFRetained::as_ptr(&value).as_ptr().cast(),
        )
    };
    err == kAXErrorSuccess
}

#[cfg(test)]
mod tests {
    use super::nonempty_ax_string;

    #[test]
    fn stubs_return_none_without_ax() {
        let selected = super::selected_text().unwrap();
        let focused = super::focused_text().unwrap();
        if cfg!(not(target_os = "macos")) {
            let applied = super::set_selected_text("hi").unwrap();
            assert!(selected.is_none());
            assert!(focused.is_none());
            assert!(!applied);
            return;
        }
        // Do not call set_selected_text when trusted — that would mutate the
        // focused UI during `cargo test`. Reads are Ok(None) when untrusted.
        if !crate::permissions::accessibility_granted() {
            assert!(selected.is_none());
            assert!(focused.is_none());
            let err = super::set_selected_text("hi").unwrap_err();
            assert!(err.contains("Accessibility"));
        }
    }

    #[test]
    fn empty_selected_text_is_none() {
        assert_eq!(nonempty_ax_string(String::new()), None);
        assert_eq!(nonempty_ax_string("hi".into()), Some("hi".into()));
    }
}
