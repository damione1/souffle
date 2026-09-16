//! Native macOS notification center integration using UNUserNotificationCenter.

#[cfg(target_os = "macos")]
use objc2::msg_send;

pub struct NativeNotification;

impl NativeNotification {
    pub fn send(title: &str, body: &str) {
        #[cfg(target_os = "macos")]
        unsafe {
            use objc2::runtime::AnyClass;
            use objc2_foundation::NSString;

            if let Some(cls) = AnyClass::get(c"UNMutableNotificationContent") {
                let content: objc2::rc::Retained<objc2::runtime::AnyObject> = msg_send![cls, new];
                let title_ns = NSString::from_str(title);
                let body_ns = NSString::from_str(body);
                let _: () = msg_send![&content, setTitle: &*title_ns];
                let _: () = msg_send![&content, setBody: &*body_ns];

                if let Some(center_cls) = AnyClass::get(c"UNUserNotificationCenter") {
                    let center: objc2::rc::Retained<objc2::runtime::AnyObject> =
                        msg_send![center_cls, currentNotificationCenter];
                    let req_cls = AnyClass::get(c"UNNotificationRequest").unwrap();
                    let id_ns = NSString::from_str("souffle-notification");
                    let null_ptr: *mut objc2::runtime::AnyObject = std::ptr::null_mut();
                    let req: objc2::rc::Retained<objc2::runtime::AnyObject> = msg_send![req_cls, requestWithIdentifier: &*id_ns, content: &*content, trigger: null_ptr];
                    let _: () = msg_send![&center, addNotificationRequest: &*req, withCompletionHandler: null_ptr];
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (title, body);
        }
    }
}
