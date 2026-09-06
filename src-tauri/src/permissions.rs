//! macOS permission detection + prompting for the startup onboarding.
//!
//! The microphone has a real read-only status API (`AVCaptureDevice`'s
//! `authorizationStatus`), so `request` checks it first and only falls back
//! to probing (briefly opening the device, which also triggers the TCC
//! prompt) when the OS hasn't decided yet. There is no equivalent for Core
//! Audio taps, so system audio is still probe-only. Accessibility (needed
//! for the synthesized Cmd+V paste) has its own cheap check
//! (`AXIsProcessTrusted`), and is granted only via System Settings, so its
//! "request" just opens the relevant pane.

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
    /// Microphone only: TCC access may well be granted, but there is no
    /// usable input device (none plugged in, or its config can't be read).
    /// Kept distinct from `Denied` because the fix isn't the same: plug in
    /// or pick a device, not open System Settings.
    NoDevice,
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

/// `AXIsProcessTrustedWithOptions`, with the option to have macOS pop the
/// native "would like to control this computer" prompt if not yet trusted.
/// Used by `repair_accessibility` to force the TCC database to (re)create
/// the entry keyed to the current binary's code signature, after
/// `tccutil reset` has cleared out a stale one.
#[cfg(target_os = "macos")]
fn accessibility_trusted_with_prompt(prompt: bool) -> bool {
    use objc2_core_foundation::{CFBoolean, CFDictionary, CFRetained, CFString};
    use std::ffi::c_void;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
        static kAXTrustedCheckOptionPrompt: *const CFString;
    }

    unsafe {
        let key: &CFString = &*kAXTrustedCheckOptionPrompt;
        let value: &CFBoolean = CFBoolean::new(prompt);
        let options: CFRetained<CFDictionary<CFString, CFBoolean>> =
            CFDictionary::from_slices(&[key], &[value]);
        AXIsProcessTrustedWithOptions(CFRetained::as_ptr(&options).as_ptr().cast())
    }
}

#[cfg(not(target_os = "macos"))]
fn accessibility_trusted_with_prompt(_prompt: bool) -> bool {
    true
}

/// The Accessibility TCC entry is keyed to the app's code-signing identity.
/// Overwriting the .app bundle in place (e.g. an in-place update) or
/// reinstalling a differently-signed build can leave a stale entry that
/// still shows as "checked" in System Settings but no longer matches, so
/// `AXIsProcessTrusted` keeps returning false. Resetting the TCC entry and
/// re-prompting lets macOS create a fresh, correctly-keyed one.
pub fn repair_accessibility() -> PermState {
    let _ = std::process::Command::new("tccutil")
        .args(["reset", "Accessibility", "com.souffle.desktop"])
        .output();

    if accessibility_trusted_with_prompt(true) {
        PermState::Granted
    } else {
        PermState::Denied
    }
}

// --- Microphone ---

/// Read-only TCC status via `AVCaptureDevice`, no prompt. Lets `request`
/// tell "the user already said no" apart from "hasn't been asked yet",
/// which the probe alone can't do.
#[cfg(target_os = "macos")]
fn microphone_authorization_status() -> PermState {
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};

    // Falling back to Unknown (rather than panicking) keeps a missing symbol
    // from taking down a permission check: the caller just probes instead.
    let Some(media_type) = (unsafe { AVMediaTypeAudio }) else {
        return PermState::Unknown;
    };
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) } {
        AVAuthorizationStatus::NotDetermined => PermState::Unknown,
        AVAuthorizationStatus::Authorized => PermState::Granted,
        // Restricted (parental controls/MDM) can't be changed from the app
        // either, so it gets the same treatment as an explicit deny.
        _ => PermState::Denied,
    }
}

#[cfg(not(target_os = "macos"))]
fn microphone_authorization_status() -> PermState {
    PermState::Unknown
}

#[cfg(target_os = "macos")]
fn open_microphone_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
        .spawn();
}

#[cfg(not(target_os = "macos"))]
fn open_microphone_settings() {}

/// Symmetric with `calendar::request_access`: an already-decided `Denied`
/// opens System Settings instead of re-running the probe, which would just
/// spend 15s to land on the same answer (macOS never re-prompts after a
/// deny). `NotDetermined` still probes, since that's what shows the TCC
/// dialog the first time.
fn request_microphone_with(
    status: impl FnOnce() -> PermState,
    open_settings: impl FnOnce(),
    probe: impl FnOnce() -> PermState,
) -> PermState {
    match status() {
        PermState::Granted => PermState::Granted,
        PermState::Denied => {
            open_settings();
            PermState::Denied
        }
        _ => probe(),
    }
}

fn request_microphone() -> PermState {
    request_microphone_with(
        microphone_authorization_status,
        open_microphone_settings,
        probe_microphone,
    )
}

fn no_op_stream_error(_e: cpal::StreamError) {}

/// Briefly open the default input device and wait for real audio callbacks.
/// This is what actually triggers the TCC prompt the first time; afterwards
/// `request_microphone` skips straight to this only when the OS reports
/// `NotDetermined`.
pub fn probe_microphone() -> PermState {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    let host = cpal::default_host();
    let Some(device) = host.default_input_device() else {
        return PermState::NoDevice;
    };
    let Ok(config) = device.default_input_config() else {
        return PermState::NoDevice;
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
    // Pause then drop so the probe's AudioUnit is actually disposed. cpal
    // 0.15 leaked StreamInner on macOS, which would leave a Bluetooth
    // headset in HFP/mono after the onboarding mic check.
    let _ = stream.pause();
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
        PermissionKind::Microphone => request_microphone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// The onboarding UI matches on this exact string (`s === "denied"`), so
    /// a rename here would silently break the repair-permission affordance.
    #[test]
    fn perm_state_denied_serializes_snake_case() {
        let json = serde_json::to_string(&PermState::Denied).unwrap();
        assert_eq!(json, "\"denied\"");
    }

    /// `NoDevice` must serialize to its own value, distinct from `Denied`:
    /// the two need different instructions in the UI (plug in a mic vs.
    /// open System Settings), so they can't collapse to the same state.
    #[test]
    fn perm_state_no_device_is_distinct_from_denied() {
        let no_device = serde_json::to_string(&PermState::NoDevice).unwrap();
        let denied = serde_json::to_string(&PermState::Denied).unwrap();
        assert_ne!(no_device, denied);
        assert_eq!(no_device, "\"no_device\"");
    }

    /// A settled `Denied` must open Settings and must NOT re-run the probe:
    /// macOS never re-prompts after a deny, so probing again would just
    /// burn 15s to land on the same answer.
    #[test]
    fn denied_opens_settings_without_probing() {
        let opened = Cell::new(false);
        let probed = Cell::new(false);

        let result = request_microphone_with(
            || PermState::Denied,
            || opened.set(true),
            || {
                probed.set(true);
                PermState::Granted
            },
        );

        assert_eq!(result, PermState::Denied);
        assert!(opened.get(), "Denied must open System Settings");
        assert!(!probed.get(), "Denied must not run the probe");
    }

    /// `NotDetermined` (modeled as `Unknown` here) still probes: that's what
    /// shows the TCC dialog the first time.
    #[test]
    fn not_determined_probes_without_opening_settings() {
        let opened = Cell::new(false);
        let probed = Cell::new(false);

        let result = request_microphone_with(
            || PermState::Unknown,
            || opened.set(true),
            || {
                probed.set(true);
                PermState::Granted
            },
        );

        assert_eq!(result, PermState::Granted);
        assert!(!opened.get(), "NotDetermined must not open Settings");
        assert!(probed.get(), "NotDetermined must run the probe");
    }

    /// Already-authorized short-circuits to `Granted` without touching
    /// Settings or the probe.
    #[test]
    fn granted_short_circuits() {
        let result = request_microphone_with(
            || PermState::Granted,
            || panic!("Granted must not open Settings"),
            || panic!("Granted must not probe"),
        );
        assert_eq!(result, PermState::Granted);
    }
}
