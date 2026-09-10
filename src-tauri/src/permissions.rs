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

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::constants::APP_IDENTIFIER;
use crate::db::Database;

/// Pause between the TCC insert call and `open` of System Settings. The
/// insert is asynchronous; opening the pane in the same turn shows a stale
/// list (empty, or a differently-signed Soufflé already ticked).
const TCC_INSERT_SETTLE: Duration = Duration::from_millis(400);

/// Last observed system-audio tap permission. There is no read-only TCC
/// API for Core Audio taps, so a successful probe is remembered and the
/// snapshot returns it without mounting a tap (SOU-120).
const SYSTEM_AUDIO_PERMISSION_KEY: &str = "system_audio_permission";

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
    pub input_monitoring: PermState,
}

/// Which capability to probe or prompt for via `request`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    Microphone,
    SystemAudio,
    Accessibility,
    Calendar,
    InputMonitoring,
}

/// Outcome of `repair_accessibility`. Distinct from `PermState` because a
/// successful `tccutil reset` plus prompt cannot observe the user's grant:
/// `AXIsProcessTrustedWithOptions` returns the *current* trust, which is
/// necessarily false a few milliseconds after the TCC entry was deleted.
#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct RepairAccessibilityResult {
    pub reset_performed: bool,
    pub prompt_shown: bool,
}

/// Cheap, non-prompting snapshot for the initial onboarding render.
///
/// System audio has no read-only TCC API. Probing mounts a Core Audio tap
/// and would prompt on a first launch, so the snapshot never probes: it
/// returns a remembered `Granted`/`Denied` after a user-initiated probe,
/// and `Unknown` until then (SOU-120).
pub fn snapshot(db: &Database) -> PermissionStatus {
    PermissionStatus {
        microphone: microphone_authorization_status(),
        system_audio: system_audio_snapshot_state(
            system_audio_supported(),
            load_remembered_system_audio(db),
        ),
        accessibility: if accessibility_granted() {
            PermState::Granted
        } else {
            PermState::Denied
        },
        // EventKit has a real read-only status API, so the snapshot is truthful
        // here (no probe needed).
        calendar: crate::calendar::authorization_state(),
        input_monitoring: if input_monitoring_granted() {
            PermState::Granted
        } else {
            PermState::Unknown
        },
    }
}

/// Snapshot value for system audio given platform support and a remembered
/// probe result. Never mounts a tap.
pub fn system_audio_snapshot_state(supported: bool, remembered: Option<PermState>) -> PermState {
    if !supported {
        return PermState::Unsupported;
    }
    match remembered {
        Some(PermState::Granted) => PermState::Granted,
        Some(PermState::Denied) => PermState::Denied,
        _ => PermState::Unknown,
    }
}

pub fn load_remembered_system_audio(db: &Database) -> Option<PermState> {
    let raw = db.get_setting(SYSTEM_AUDIO_PERMISSION_KEY).ok().flatten()?;
    match serde_json::from_str::<PermState>(&raw) {
        Ok(state @ (PermState::Granted | PermState::Denied)) => Some(state),
        _ => None,
    }
}

pub fn remember_system_audio(db: &Database, state: PermState) {
    if !matches!(state, PermState::Granted | PermState::Denied) {
        return;
    }
    if let Ok(raw) = serde_json::to_string(&state)
        && let Err(e) = db.set_setting(SYSTEM_AUDIO_PERMISSION_KEY, &raw)
    {
        tracing::warn!(error = %e, "Failed to persist system audio permission");
    }
}

fn system_audio_supported() -> bool {
    crate::platform::system_audio_capture_supported()
}

/// TCC prompts (`AXIsProcessTrustedWithOptions`, `CGRequestListenEventAccess`,
/// `IOHIDRequestAccess`) only insert this process into System Settings when
/// they run on the main thread. `request_permission` is `spawn_blocking`, so
/// hop. Reads (`AXIsProcessTrusted`, `CGPreflightListenEventAccess`) stay on
/// the caller: `cargo test` has no main runloop, and a `dispatch_sync` there
/// hangs (SOU-122 AC3).
///
/// Main-thread is necessary but not sufficient after an in-place rebuild:
/// Launch Services must point at *this* binary, Input Monitoring needs
/// `NSInputMonitoringUsageDescription`, and System Settings must not open
/// before TCC has committed the new row.
#[cfg(target_os = "macos")]
fn on_main<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    on_main_with(is_main_thread(), f)
}

#[cfg(target_os = "macos")]
fn on_main_with<R: Send>(already_main: bool, f: impl FnOnce() -> R + Send) -> R {
    if already_main {
        return f();
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    dispatch2::DispatchQueue::main().exec_sync(move || {
        let _ = tx.send(f());
    });
    rx.recv().expect("main queue dropped a TCC hop")
}

#[cfg(target_os = "macos")]
fn is_main_thread() -> bool {
    unsafe extern "C" {
        fn pthread_main_np() -> i32;
    }
    unsafe { pthread_main_np() != 0 }
}

/// Walk `.../Name.app/Contents/MacOS/<exe>` up to the `.app` bundle.
/// A bare debug binary (`target/debug/souffle`) has no bundle to register.
fn app_bundle_path_from_exe(exe: &Path) -> Option<&Path> {
    let macos_dir = exe.parent()?;
    if macos_dir.file_name()?.to_str() != Some("MacOS") {
        return None;
    }
    let contents = macos_dir.parent()?;
    if contents.file_name()?.to_str() != Some("Contents") {
        return None;
    }
    let app = contents.parent()?;
    if app.extension()?.to_str() != Some("app") {
        return None;
    }
    Some(app)
}

fn current_app_bundle_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    app_bundle_path_from_exe(&exe).map(Path::to_path_buf)
}

/// Re-register this `.app` with Launch Services so TCC / System Settings
/// resolve the current binary after an in-place rebuild. No-op outside a
/// bundle, and no-op off macOS. Safe to call more than once.
pub fn register_with_launch_services() {
    #[cfg(target_os = "macos")]
    register_current_bundle_with_launch_services();
}

#[cfg(target_os = "macos")]
fn register_current_bundle_with_launch_services() {
    use core_foundation::base::TCFType;
    use core_foundation::url::CFURL;

    let Some(path) = current_app_bundle_path() else {
        return;
    };
    // Apple's recipe for an overwritten bundle: bump mtime, then
    // LSRegisterURL(..., true), so the next TCC lookup is this binary.
    let _ = std::fs::File::open(&path).and_then(|f| f.set_modified(std::time::SystemTime::now()));
    let Some(url) = CFURL::from_path(&path, true) else {
        tracing::warn!(path = %path.display(), "CFURL for app bundle failed");
        return;
    };
    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn LSRegisterURL(in_url: core_foundation::url::CFURLRef, in_update: u8) -> i32;
    }
    let status = unsafe { LSRegisterURL(url.as_concrete_TypeRef(), 1) };
    if status != 0 {
        tracing::warn!(status, path = %path.display(), "LSRegisterURL failed");
    }
}

fn wait_for_tcc_insert() {
    if cfg!(test) {
        return;
    }
    std::thread::sleep(TCC_INSERT_SETTLE);
}

fn open_privacy_pane(pane: &str) {
    let _ = std::process::Command::new("open")
        .arg(format!(
            "x-apple.systempreferences:com.apple.preference.security?{pane}"
        ))
        .spawn();
}

/// Prompt (and any Launch Services registration) first, let TCC commit the
/// row, then open the pane. Opening in the same turn shows a stale list.
fn prompt_then_open_settings(prompt: impl FnOnce(), wait: impl FnOnce(), open: impl FnOnce()) {
    prompt();
    wait();
    open();
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
    prompt_then_open_settings(
        || {
            on_main(|| {
                let _ = accessibility_trusted_with_prompt_now(true);
            });
        },
        wait_for_tcc_insert,
        || open_privacy_pane("Privacy_Accessibility"),
    );
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
    on_main(move || accessibility_trusted_with_prompt_now(prompt))
}

#[cfg(target_os = "macos")]
fn accessibility_trusted_with_prompt_now(prompt: bool) -> bool {
    use objc2_core_foundation::{CFBoolean, CFDictionary, CFRetained, CFString};
    use std::ffi::c_void;

    register_current_bundle_with_launch_services();

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
///
/// Returns what the repair *did*, not a permission verdict. The AX check
/// after a successful reset is almost always false (the user hasn't ticked
/// the fresh prompt yet); treating that as `Denied` made the UI claim every
/// repair had failed, including the ones that worked (SOU-054).
pub fn repair_accessibility() -> Result<RepairAccessibilityResult, String> {
    repair_accessibility_with(
        APP_IDENTIFIER,
        tccutil_reset_accessibility,
        accessibility_trusted_with_prompt,
    )
}

fn tccutil_reset_accessibility(bundle_id: &str) -> Result<(), String> {
    let output = std::process::Command::new("tccutil")
        .args(["reset", "Accessibility", bundle_id])
        .output()
        .map_err(|e| format!("Failed to run tccutil: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let code = output.status.code().unwrap_or(-1);
    tracing::error!(bundle_id, code, %stderr, "tccutil reset Accessibility failed");
    Err(if stderr.is_empty() {
        format!("tccutil reset Accessibility failed (exit {code})")
    } else {
        format!("tccutil reset Accessibility failed (exit {code}): {stderr}")
    })
}

fn repair_accessibility_with(
    bundle_id: &str,
    reset: impl FnOnce(&str) -> Result<(), String>,
    prompt: impl FnOnce(bool) -> bool,
) -> Result<RepairAccessibilityResult, String> {
    reset(bundle_id)?;
    // Always ask macOS to show the prompt. The boolean it returns is the
    // *current* trust, not whether the user accepted — ignore it as a verdict.
    let _trusted = prompt(true);
    Ok(RepairAccessibilityResult {
        reset_performed: true,
        prompt_shown: true,
    })
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

fn request_microphone_with(
    status: impl FnOnce() -> PermState,
    open_settings: impl FnOnce(),
    probe: impl FnOnce() -> PermState,
    has_device: impl FnOnce() -> bool,
) -> PermState {
    match status() {
        PermState::Granted => {
            if has_device() {
                PermState::Granted
            } else {
                PermState::NoDevice
            }
        }
        PermState::Denied => {
            open_settings();
            PermState::Denied
        }
        _ => probe(),
    }
}

fn has_microphone_device() -> bool {
    use cpal::traits::HostTrait;
    cpal::default_host().default_input_device().is_some()
}

fn request_microphone() -> PermState {
    request_microphone_with(
        microphone_authorization_status,
        open_microphone_settings,
        probe_microphone,
        has_microphone_device,
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

    let stream = match stream {
        Ok(s) => s,
        Err(cpal::BuildStreamError::DeviceNotAvailable) => return PermState::NoDevice,
        Err(_) => return PermState::Denied,
    };
    if let Err(e) = stream.play() {
        match e {
            cpal::PlayStreamError::DeviceNotAvailable => return PermState::NoDevice,
            _ => return PermState::Denied,
        }
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
        PermissionKind::InputMonitoring => {
            open_input_monitoring_settings();
            if input_monitoring_granted() {
                PermState::Granted
            } else {
                PermState::Denied
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
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

    #[test]
    fn on_main_runs_inline_when_already_on_the_main_thread() {
        // cargo test worker threads have no dispatch runloop. Hopping
        // (`already_main: false`) would hang; the production path uses
        // `pthread_main_np` so a test must only exercise the inline arm.
        let ran = std::sync::atomic::AtomicBool::new(false);
        on_main_with(true, || {
            ran.store(true, std::sync::atomic::Ordering::SeqCst)
        });
        assert!(ran.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn tcc_prompt_runs_before_the_settings_pane_opens() {
        use std::cell::RefCell;
        let order = RefCell::new(Vec::new());
        prompt_then_open_settings(
            || order.borrow_mut().push("prompt"),
            || order.borrow_mut().push("wait"),
            || order.borrow_mut().push("open"),
        );
        assert_eq!(
            *order.borrow(),
            ["prompt", "wait", "open"],
            "System Settings must not open in the same turn as the TCC insert"
        );
    }

    #[test]
    fn app_bundle_path_walks_contents_macos() {
        let exe = Path::new("/tmp/Soufflé.app/Contents/MacOS/souffle");
        assert_eq!(
            app_bundle_path_from_exe(exe),
            Some(Path::new("/tmp/Soufflé.app"))
        );
    }

    #[test]
    fn app_bundle_path_rejects_a_bare_debug_binary() {
        assert_eq!(
            app_bundle_path_from_exe(Path::new("/tmp/target/debug/souffle")),
            None
        );
        assert_eq!(
            app_bundle_path_from_exe(Path::new("/tmp/Soufflé.app/Contents/MacOS")),
            None,
            "the MacOS directory itself is not the executable"
        );
    }

    #[test]
    fn info_plist_declares_input_monitoring_usage() {
        let plist = std::fs::read_to_string(format!("{}/Info.plist", env!("CARGO_MANIFEST_DIR")))
            .expect("src-tauri/Info.plist");
        assert!(
            plist.contains("<key>NSInputMonitoringUsageDescription</key>"),
            "IOHIDRequestAccess does not insert the Input Monitoring row without this key"
        );
        assert!(
            plist.contains("keyboard and mouse events"),
            "usage string should describe Input Monitoring, not a different permission"
        );
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
            || true,
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
            || true,
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
            || true,
        );
        assert_eq!(result, PermState::Granted);
    }

    #[test]
    fn repair_reports_reset_not_denied_when_ax_is_still_false() {
        let prompted = Cell::new(false);
        let result = repair_accessibility_with(
            APP_IDENTIFIER,
            |id| {
                assert_eq!(id, APP_IDENTIFIER);
                Ok(())
            },
            |prompt| {
                assert!(prompt, "repair must request the system prompt");
                prompted.set(true);
                false
            },
        )
        .expect("successful tccutil must not become an error");

        assert!(result.reset_performed);
        assert!(result.prompt_shown);
        assert!(prompted.get());
    }

    #[test]
    fn repair_surfaces_tccutil_failure_and_skips_the_prompt() {
        let prompted = Cell::new(false);
        let result = repair_accessibility_with(
            "com.example.unknown",
            |id| {
                Err(format!(
                    "tccutil reset Accessibility failed (exit 64): No such bundle identifier \"{id}\""
                ))
            },
            |_| {
                prompted.set(true);
                false
            },
        );

        assert!(result.is_err(), "a failed reset must not look like success");
        let err = result.unwrap_err();
        assert!(err.contains("64"), "{err}");
        assert!(err.contains("com.example.unknown"), "{err}");
        assert!(!prompted.get(), "do not prompt after a failed reset");
    }

    #[test]
    fn repair_uses_the_app_bundle_id() {
        let mut seen = None;
        let _ = repair_accessibility_with(
            APP_IDENTIFIER,
            |id| {
                seen = Some(id.to_string());
                Ok(())
            },
            |_| false,
        );
        assert_eq!(seen.as_deref(), Some(APP_IDENTIFIER));
        assert_eq!(APP_IDENTIFIER, "com.souffle.desktop");
    }

    #[test]
    fn system_audio_snapshot_is_unknown_until_a_probe_succeeds() {
        assert_eq!(system_audio_snapshot_state(true, None), PermState::Unknown);
        assert_eq!(
            system_audio_snapshot_state(true, Some(PermState::Unknown)),
            PermState::Unknown
        );
    }

    #[test]
    fn system_audio_snapshot_returns_remembered_granted_without_probing() {
        assert_eq!(
            system_audio_snapshot_state(true, Some(PermState::Granted)),
            PermState::Granted
        );
    }

    #[test]
    fn system_audio_snapshot_returns_remembered_denied() {
        assert_eq!(
            system_audio_snapshot_state(true, Some(PermState::Denied)),
            PermState::Denied
        );
    }

    #[test]
    fn system_audio_snapshot_stays_unsupported_even_if_something_was_remembered() {
        assert_eq!(
            system_audio_snapshot_state(false, Some(PermState::Granted)),
            PermState::Unsupported
        );
        assert_eq!(
            system_audio_snapshot_state(false, None),
            PermState::Unsupported
        );
    }

    #[test]
    fn system_audio_permission_round_trips_through_the_db() {
        let (db, _dir) = crate::test_helpers::fixtures::test_db();
        assert_eq!(load_remembered_system_audio(&db), None);

        remember_system_audio(&db, PermState::Granted);
        assert_eq!(load_remembered_system_audio(&db), Some(PermState::Granted));
        assert_eq!(
            snapshot(&db).system_audio,
            if system_audio_supported() {
                PermState::Granted
            } else {
                PermState::Unsupported
            }
        );

        remember_system_audio(&db, PermState::Denied);
        assert_eq!(load_remembered_system_audio(&db), Some(PermState::Denied));

        remember_system_audio(&db, PermState::Unknown);
        assert_eq!(
            load_remembered_system_audio(&db),
            Some(PermState::Denied),
            "Unknown must not clobber a remembered answer"
        );
    }
}

#[cfg(target_os = "macos")]
fn input_monitoring_granted() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightListenEventAccess() -> bool;
        #[allow(dead_code)]
        fn CGRequestListenEventAccess() -> bool;
    }
    unsafe { CGPreflightListenEventAccess() }
}

#[cfg(not(target_os = "macos"))]
fn input_monitoring_granted() -> bool {
    true
}

#[cfg(target_os = "macos")]
fn request_listen_event_access_now() {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGRequestListenEventAccess() -> bool;
    }
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOHIDRequestAccess(request_type: u32) -> bool;
    }
    register_current_bundle_with_launch_services();
    // kIOHIDRequestTypeListenEvent. DTS: this is the API that inserts the
    // Input Monitoring row. CGRequestListenEventAccess is the older
    // CoreGraphics equivalent. A listen-only tap is the actual use of the
    // privilege; creating one (even if it fails) is what some macOS
    // versions need before the row appears. Dropped immediately, never
    // attached to a runloop.
    const LISTEN_EVENT: u32 = 1;
    unsafe {
        let _ = IOHIDRequestAccess(LISTEN_EVENT);
        let _ = CGRequestListenEventAccess();
    }
    use core_graphics::event::{
        CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
        CallbackResult,
    };
    let _ = CGEventTap::new(
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        vec![CGEventType::KeyDown],
        |_proxy, _ty, _event| CallbackResult::Keep,
    );
}

#[cfg(target_os = "macos")]
fn open_input_monitoring_settings() {
    prompt_then_open_settings(
        || {
            on_main(request_listen_event_access_now);
        },
        wait_for_tcc_insert,
        || open_privacy_pane("Privacy_ListenEvent"),
    );
}

#[cfg(not(target_os = "macos"))]
fn open_input_monitoring_settings() {}
