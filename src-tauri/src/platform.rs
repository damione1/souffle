/// Drain Metal autorelease pool to prevent GPU memory leak.
/// Candle's Metal backend creates autoreleased ObjC objects per tensor operation.
/// Without periodic draining, these accumulate and corrupt GPU state after ~3 sessions.
/// See: https://github.com/huggingface/candle/issues/2271
#[cfg(target_os = "macos")]
pub fn with_autorelease_pool<T, F: FnOnce() -> T>(f: F) -> T {
    unsafe extern "C" {
        fn objc_autoreleasePoolPush() -> *mut std::ffi::c_void;
        fn objc_autoreleasePoolPop(pool: *mut std::ffi::c_void);
    }
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let result = f();
        objc_autoreleasePoolPop(pool);
        result
    }
}

#[cfg(not(target_os = "macos"))]
pub fn with_autorelease_pool<T, F: FnOnce() -> T>(f: F) -> T {
    f()
}

/// Whether Core Audio process taps are available (macOS 14.4+).
/// All tap calls must be gated behind this check — the symbols are weakly
/// linked and crash on older systems.
#[cfg(target_os = "macos")]
pub fn system_audio_capture_supported() -> bool {
    use objc2_foundation::{NSOperatingSystemVersion, NSProcessInfo};

    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
        majorVersion: 14,
        minorVersion: 4,
        patchVersion: 0,
    })
}

#[cfg(not(target_os = "macos"))]
pub fn system_audio_capture_supported() -> bool {
    false
}
