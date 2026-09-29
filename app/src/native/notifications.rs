//! Native replacement for `tauri-plugin-notification` (SOU-191).
//!
//! `UNUserNotificationCenter` via `objc2`, the same FFI approach already used
//! for the tray/pill (`objc2-app-kit`) elsewhere in this crate. Authorization
//! must be requested once before any notification can show; a denial just
//! means `show` silently does nothing, exactly like today (the Tauri plugin
//! never surfaced denial to the caller either — see the two call sites,
//! neither checks a return value for that reason).

use objc2::rc::Retained;
use objc2_foundation::NSString;
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
    UNUserNotificationCenter,
};

/// Ask the user for notification authorization. Call once at startup
/// (mirrors the Tauri plugin's own lazy-authorize-on-first-use, made
/// explicit here since there is no plugin lifecycle to hook it into).
pub fn request_authorization() {
    let center = UNUserNotificationCenter::currentNotificationCenter();
    let options = UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound;
    center.requestAuthorizationWithOptions_completionHandler(
        options,
        &block2::StackBlock::new(|_granted: objc2::runtime::Bool, _error| {}).copy(),
    );
}

/// Show a plain title+body notification. Fire-and-forget: neither call site
/// in this codebase acted on success/failure before this ticket either.
pub fn notify(title: &str, body: &str) {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));

    let identifier = NSString::from_str(&uuid::Uuid::new_v4().to_string());
    let request: Retained<UNNotificationRequest> =
        UNNotificationRequest::requestWithIdentifier_content_trigger(&identifier, &content, None);

    let center = UNUserNotificationCenter::currentNotificationCenter();
    center.addNotificationRequest_withCompletionHandler(
        &request,
        Some(&block2::StackBlock::new(|_error| {}).copy()),
    );
}
