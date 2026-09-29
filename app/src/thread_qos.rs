//! macOS thread quality-of-service (QoS) class configuration.
//!
//! On macOS, worker threads should be tagged with explicit QoS classes so the
//! kernel scheduler respects priority hierarchies under CPU contention. Inference
//! threads use UserInitiated (not Utility) deliberately: Utility can land work on
//! efficiency cores and fall behind real-time requirements, while UserInitiated
//! still yields to the UI's UserInteractive work but stays on performance cores.

pub enum ThreadQos {
    /// Highest priority tier. The audio capture thread uses this: it mixes
    /// live audio on a 5ms tick during meetings and must never miss frames.
    UserInteractive,
    /// One tier below UserInteractive. Used for inference: yields to audio
    /// and UI under contention but stays eligible for performance cores.
    UserInitiated,
}

/// Set the current thread's macOS QoS class. On non-macOS, this is a no-op.
///
/// Returns true if the call succeeded (or if called on non-macOS).
/// Logs a debug message if the underlying libc call returns nonzero.
pub fn set_current_thread_qos(qos: ThreadQos) -> bool {
    #[cfg(target_os = "macos")]
    {
        let qos_class = match qos {
            ThreadQos::UserInteractive => libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE,
            ThreadQos::UserInitiated => libc::qos_class_t::QOS_CLASS_USER_INITIATED,
        };

        let result = unsafe { libc::pthread_set_qos_class_self_np(qos_class, 0) };
        if result != 0 {
            tracing::debug!(
                "pthread_set_qos_class_self_np returned {}: thread may not have QoS class set",
                result
            );
            false
        } else {
            true
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        // On non-macOS platforms, no-op.
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_user_interactive_qos_no_panic() {
        let success = set_current_thread_qos(ThreadQos::UserInteractive);
        assert!(success, "UserInteractive QoS should succeed");
    }

    #[test]
    fn set_user_initiated_qos_no_panic() {
        let success = set_current_thread_qos(ThreadQos::UserInitiated);
        assert!(success, "UserInitiated QoS should succeed");
    }

    #[test]
    fn set_qos_on_scratch_thread() {
        let handle = std::thread::spawn(|| {
            let _ = set_current_thread_qos(ThreadQos::UserInitiated);
        });
        handle.join().expect("Thread should not panic");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn verify_qos_class_applied() {
        use std::sync::{Arc, Mutex};
        let result: Arc<Mutex<Option<libc::qos_class_t>>> = Arc::new(Mutex::new(None));
        let result_ref = Arc::clone(&result);

        let handle = std::thread::spawn(move || {
            let success = set_current_thread_qos(ThreadQos::UserInitiated);
            assert!(success, "Set should succeed");

            // Verify the class was actually applied using the getter
            let mut class: libc::qos_class_t = libc::qos_class_t::QOS_CLASS_UNSPECIFIED;
            let mut priority: libc::c_int = 0;
            let ret = unsafe {
                libc::pthread_get_qos_class_np(libc::pthread_self(), &mut class, &mut priority)
            };

            if ret == 0 {
                *result_ref.lock().unwrap() = Some(class);
            }
        });

        handle.join().expect("Thread should not panic");

        if let Some(class) = *result.lock().unwrap() {
            // The getter should return the UserInitiated class we set
            assert_eq!(
                class as u32,
                libc::qos_class_t::QOS_CLASS_USER_INITIATED as u32,
                "QoS class should be UserInitiated after setting"
            );
        } else {
            // If the getter isn't available or failed, the smoke test is sufficient
            tracing::debug!("QoS class verification skipped (getter unavailable)");
        }
    }
}
