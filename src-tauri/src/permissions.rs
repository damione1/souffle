//! macOS permission detection + prompting for the startup onboarding.
//!
//! There is no clean read-only status API for the microphone or Core Audio
//! taps without pulling in AVFoundation, so we *probe*: briefly open the
//! device. Opening it both triggers the system TCC prompt (first time) and
//! tells us whether audio actually flows (granted). Accessibility — needed for
//! the synthesized Cmd+V paste — does have a cheap check (`AXIsProcessTrusted`),
//! and is granted only via System Settings, so its "request" just opens the
//! relevant pane.

use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermState {
    Granted,
    Denied,
    /// Not yet probed — the user hasn't triggered this one (probing would
    /// prompt, so we don't do it unsolicited at startup).
    Unknown,
    /// The OS doesn't support this capability (e.g. taps need macOS 14.4+).
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PermissionStatus {
    pub microphone: PermState,
    pub system_audio: PermState,
    pub accessibility: PermState,
    pub calendar: PermState,
}

/// Which capability to probe or prompt for via `request`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    Microphone,
    SystemAudio,
    Accessibility,
    Calendar,
}

/// Cheap, non-prompting snapshot for the initial onboarding render. Microphone
/// and system audio are left `Unknown` (probing them would prompt); the user
/// triggers those explicitly via `request_permission`.
pub fn snapshot() -> PermissionStatus {
    PermissionStatus {
        microphone: PermState::Unknown,
        system_audio: if system_audio_supported() {
            PermState::Unknown
        } else {
            PermState::Unsupported
        },
        accessibility: if accessibility_granted() {
            PermState::Granted
        } else {
            PermState::Denied
        },
        // EventKit has a real read-only status API, so the snapshot is truthful
        // here (no probe needed).
        calendar: crate::calendar::authorization_state(),
    }
}

fn system_audio_supported() -> bool {
    crate::platform::system_audio_capture_supported()
}

// --- Accessibility (synthesized Cmd+V paste) ---

#[cfg(target_os = "macos")]
pub fn accessibility_granted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}

#[cfg(not(target_os = "macos"))]
pub fn accessibility_granted() -> bool {
    true
}

#[cfg(target_os = "macos")]
fn open_accessibility_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}

#[cfg(not(target_os = "macos"))]
fn open_accessibility_settings() {}

// --- Microphone (probe: triggers the TCC prompt + detects delivery) ---

fn no_op_stream_error(_e: cpal::StreamError) {}

pub fn probe_microphone() -> PermState {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    let host = cpal::default_host();
    let Some(device) = host.default_input_device() else {
        return PermState::Denied;
    };
    let Ok(config) = device.default_input_config() else {
        return PermState::Denied;
    };

    let got = Arc::new(AtomicBool::new(false));
    let sample_format = config.sample_format();
    let stream_config: cpal::StreamConfig = config.into();

    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let got = Arc::clone(&got);
            device.build_input_stream(
                &stream_config,
                move |_d: &[f32], _: &_| got.store(true, Ordering::Relaxed),
                no_op_stream_error,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let got = Arc::clone(&got);
            device.build_input_stream(
                &stream_config,
                move |_d: &[i16], _: &_| got.store(true, Ordering::Relaxed),
                no_op_stream_error,
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let got = Arc::clone(&got);
            device.build_input_stream(
                &stream_config,
                move |_d: &[u16], _: &_| got.store(true, Ordering::Relaxed),
                no_op_stream_error,
                None,
            )
        }
        _ => return PermState::Denied,
    };

    let Ok(stream) = stream else {
        return PermState::Denied;
    };
    if stream.play().is_err() {
        return PermState::Denied;
    }

    // Wait up to 15s for a callback. On first launch the macOS TCC dialog is
    // still on screen when this probe starts, so the window must outlast the
    // time it takes the user to read it and click Allow/Deny. When permission
    // was already granted the early exit as soon as data arrives keeps this fast.
    for _ in 0..150 {
        if got.load(Ordering::Relaxed) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(stream);

    if got.load(Ordering::Relaxed) {
        PermState::Granted
    } else {
        PermState::Denied
    }
}

// --- System audio (probe via a short-lived Core Audio tap) ---

#[cfg(target_os = "macos")]
pub fn probe_system_audio() -> PermState {
    use ringbuf::HeapRb;
    use ringbuf::traits::Split;
    use std::time::Duration;

    if !system_audio_supported() {
        return PermState::Unsupported;
    }
    let (prod, _cons) = HeapRb::<f32>::new(crate::audio::mixer::MIX_RATE as usize).split();
    match crate::audio::system_tap::spawn_tap(prod, Duration::from_secs(2)) {
        Ok(_tap) => PermState::Granted, // dropping the handle tears the tap down
        Err(_) => PermState::Denied,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn probe_system_audio() -> PermState {
    PermState::Unsupported
}

/// Trigger the native prompt (or open Settings) for one permission and return
/// the resulting state.
pub fn request(kind: PermissionKind) -> PermState {
    match kind {
        PermissionKind::Microphone => probe_microphone(),
        PermissionKind::SystemAudio => probe_system_audio(),
        PermissionKind::Accessibility => {
            open_accessibility_settings();
            if accessibility_granted() {
                PermState::Granted
            } else {
                PermState::Denied
            }
        }
        PermissionKind::Calendar => crate::calendar::request_access(),
    }
}
