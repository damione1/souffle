use crate::ax_text;
use crate::frontmost;

/// Localized name of the frontmost app at call time.
pub fn frontmost_app_name() -> Result<Option<String>, String> {
    Ok(frontmost::localized_name())
}

/// Selected text in the focused accessibility element, if any.
pub fn read_selected_text() -> Result<Option<String>, String> {
    ax_text::selected_text()
}

/// Full value of the focused accessibility element, if readable.
pub fn read_focused_text() -> Result<Option<String>, String> {
    ax_text::focused_text()
}
