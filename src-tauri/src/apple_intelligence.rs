use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};

#[repr(C)]
pub struct AppleLLMResponse {
    pub response: *mut c_char,
    pub success: c_int,
    pub error_message: *mut c_char,
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
unsafe extern "C" {
    fn is_apple_intelligence_available() -> c_int;
    fn apple_intelligence_unavailable_reason() -> *mut c_char;
    fn process_text_with_system_prompt_apple(
        system_prompt: *const c_char,
        user_content: *const c_char,
        max_tokens: i32,
    ) -> *mut AppleLLMResponse;
    fn free_apple_llm_response(response: *mut AppleLLMResponse);
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn is_apple_intelligence_available() -> c_int {
    0
}

/// Whether this build linked the Apple Intelligence stub instead of FoundationModels.
pub fn is_stub_linked() -> bool {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        cfg!(apple_intelligence_stub)
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        true
    }
}

/// Whether Apple Intelligence is available on this device at runtime.
pub fn check_apple_intelligence_availability() -> bool {
    if is_stub_linked() {
        return false;
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        unsafe { is_apple_intelligence_available() == 1 }
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        false
    }
}

/// Reason Apple Intelligence is unavailable on this device, or `None` when available.
///
/// On non-macOS/non-Apple-Silicon builds this always reports `"unsupported_platform"`.
pub fn unavailable_reason() -> Option<String> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let reason_ptr = unsafe { apple_intelligence_unavailable_reason() };
        if reason_ptr.is_null() {
            return None;
        }
        let reason = unsafe { CStr::from_ptr(reason_ptr) }
            .to_string_lossy()
            .into_owned();
        unsafe { libc::free(reason_ptr.cast()) };
        Some(reason)
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        Some("unsupported_platform".to_string())
    }
}

/// Run one on-device Foundation Models generation with separate system and user prompts.
pub fn process_text_with_system_prompt(
    system_prompt: &str,
    user_content: &str,
    max_tokens: i32,
) -> Result<String, String> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let system_cstr = CString::new(system_prompt).map_err(|e| e.to_string())?;
        let user_cstr = CString::new(user_content).map_err(|e| e.to_string())?;

        let response_ptr = unsafe {
            process_text_with_system_prompt_apple(
                system_cstr.as_ptr(),
                user_cstr.as_ptr(),
                max_tokens,
            )
        };

        if response_ptr.is_null() {
            return Err("Null response from Apple Intelligence".into());
        }

        let response = unsafe { &*response_ptr };

        let result = if response.success == 1 {
            if response.response.is_null() {
                Ok(String::new())
            } else {
                let c_str = unsafe { CStr::from_ptr(response.response) };
                Ok(c_str.to_string_lossy().into_owned())
            }
        } else {
            let error_c_str = if !response.error_message.is_null() {
                unsafe { CStr::from_ptr(response.error_message) }
            } else {
                c"Unknown error"
            };
            Err(error_c_str.to_string_lossy().into_owned())
        };

        unsafe { free_apple_llm_response(response_ptr) };

        result
    }

    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (system_prompt, user_content, max_tokens);
        Err("Apple Intelligence is only supported on Apple Silicon macOS".into())
    }
}

#[cfg(test)]
mod tests {
    use super::{check_apple_intelligence_availability, is_stub_linked, unavailable_reason};

    #[test]
    fn availability_check_does_not_panic() {
        let _available = check_apple_intelligence_availability();
    }

    #[test]
    fn stub_build_never_reports_available() {
        if is_stub_linked() {
            assert!(!check_apple_intelligence_availability());
        }
    }

    #[test]
    fn unavailable_reason_is_coherent_with_availability() {
        let available = check_apple_intelligence_availability();
        let reason = unavailable_reason();
        assert_eq!(available, reason.is_none());
        if is_stub_linked() {
            assert_eq!(reason.as_deref(), Some("stub"));
        }
    }
}
