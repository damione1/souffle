//! macOS permission detection + prompting for the startup onboarding.
//!
//! Reading a status never opens a device. The microphone answers from
//! `AVCaptureDevice`'s `authorizationStatus` and is asked for through
//! `requestAccessForMediaType:completionHandler:`, whose completion block is
//! the only in-process signal that carries a fresh answer. System audio is
//! read through TCC's `TCCAccessPreflight` SPI and asked for by mounting the
//! tap. Accessibility (needed for the synthesized Cmd+V paste and for the
//! native single-key shortcut tap) has its own cheap check
//! (`AXIsProcessTrusted`), and is granted only via System Settings, so its
//! "request" just opens the relevant pane.
//!
//! Input Monitoring is deliberately absent. An active `CGEventTap` is
//! authorized by Accessibility, which subsumes the listen right, so the
//! permission gated nothing; and once the app has an Accessibility TCC
//! record, tccd answers a ListenEvent request by composing from that parent
//! record, so asking could not even create the row it promised.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use specta::Type;

/// Pause between the TCC insert call and `open` of System Settings. The
/// insert is asynchronous; opening the pane in the same turn shows a stale
/// list (empty, or a differently-signed Soufflé already ticked).
const TCC_INSERT_SETTLE: Duration = Duration::from_millis(400);

/// A microphone grant reported by the `requestAccess` completion block.
///
/// `AVCaptureDevice`'s `authorizationStatus` is answered from a per-process
/// cache filled on the first call. AVFoundation's own request path refreshes
/// it, so this flag should never be needed; it exists because the panel must
/// not be able to stick on "Grant" if it ever is. In this process only: a
/// grant is not a fact the app owns, and a persisted copy could only go on
/// claiming a permission the user has since revoked.
static MICROPHONE_GRANT_OBSERVED: AtomicBool = AtomicBool::new(false);

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

/// Outcome of `repair_accessibility`. Distinct from `PermState` because a
/// successful `tccutil reset` plus prompt cannot observe the user's grant:
/// `AXIsProcessTrustedWithOptions` returns the *current* trust, which is
/// necessarily false a few milliseconds after the TCC entry was deleted.
#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct RepairAccessibilityResult {
    pub reset_performed: bool,
    pub prompt_shown: bool,
}

/// Cheap, non-prompting snapshot for the initial onboarding render. No
/// entry in it opens a device: every capability answers from a status API.
pub fn snapshot() -> PermissionStatus {
    PermissionStatus {
        microphone: microphone_snapshot_state(
            microphone_authorization_status(),
            MICROPHONE_GRANT_OBSERVED.load(Ordering::Relaxed),
        ),
        system_audio: system_audio_status(),
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

/// Live system-audio status, read without creating any audio object.
pub fn system_audio_status() -> PermState {
    system_audio_state_with(
        system_audio_supported(),
        audio_capture_preflight,
        probe_system_audio,
    )
}

/// `probe` only runs when the TCC SPI is unavailable (feature off, or the
/// symbol gone from a future macOS). It answers by mounting a tap, which is
/// what the read used to do and what the SPI exists to avoid, but it is an
/// answer rather than a panic.
pub fn system_audio_state_with(
    supported: bool,
    preflight: impl FnOnce() -> Option<PermState>,
    probe: impl FnOnce() -> PermState,
) -> PermState {
    if !supported {
        return PermState::Unsupported;
    }
    preflight().unwrap_or_else(probe)
}

/// Snapshot value for the microphone. The live status wins whenever the OS
/// has decided; a grant seen by this process covers the one case it cannot
/// answer, a cached `NotDetermined` taken before the user said yes.
pub fn microphone_snapshot_state(live: PermState, observed_grant: bool) -> PermState {
    match live {
        PermState::Unknown if observed_grant => PermState::Granted,
        state => state,
    }
}

fn system_audio_supported() -> bool {
    crate::platform::system_audio_capture_supported()
}

/// An `AXIsProcessTrustedWithOptions` prompt only registers this process
/// with System Settings when it runs on the main thread. `request_permission`
/// is `spawn_blocking`, so hop. Reads (`AXIsProcessTrusted`) stay on the
/// caller: `cargo test` has no main runloop, and a `dispatch_sync` there
/// hangs (SOU-122 AC3).
///
/// Main-thread is necessary but not sufficient after an in-place rebuild:
/// Launch Services must point at *this* binary, and System Settings must
/// not open before TCC has committed the new row.
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
    let bundle_id = crate::constants::running_app_identifier();
    // The only trace this path leaves. Without it a repair that never runs
    // and a repair that runs and changes nothing look identical from the
    // outside, and both are reported as "the button does nothing".
    tracing::info!(bundle_id, "Repairing the Accessibility permission");
    tccutil_reset_service("Accessibility", &bundle_id)?;
    tracing::info!("Accessibility TCC entry reset; asking macOS to prompt");
    let _ = accessibility_trusted_with_prompt(true);
    Ok(RepairAccessibilityResult {
        reset_performed: true,
        prompt_shown: true,
    })
}

fn tccutil_reset_service(service: &str, bundle_id: &str) -> Result<(), String> {
    let output = std::process::Command::new("tccutil")
        .args(["reset", service, bundle_id])
        .output()
        .map_err(|e| format!("Failed to run tccutil: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let code = output.status.code().unwrap_or(-1);
    tracing::error!(bundle_id, service, code, %stderr, "tccutil reset failed");
    Err(if stderr.is_empty() {
        format!("tccutil reset {service} failed (exit {code})")
    } else {
        format!("tccutil reset {service} failed (exit {code}): {stderr}")
    })
}

#[cfg(test)]
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

/// `Denied` also covers `Restricted` (parental controls, MDM): macOS never
/// re-prompts once it has an answer, so the only move left is the pane.
fn request_microphone_with(
    status: impl FnOnce() -> PermState,
    open_settings: impl FnOnce(),
    request_access: impl FnOnce() -> bool,
    has_device: impl FnOnce() -> bool,
) -> PermState {
    match status() {
        PermState::Granted => granted_or_no_device(has_device),
        PermState::Denied => {
            open_settings();
            PermState::Denied
        }
        _ => {
            if request_access() {
                granted_or_no_device(has_device)
            } else {
                PermState::Denied
            }
        }
    }
}

/// TCC access and a usable input device are different problems with
/// different fixes, so a grant with nothing plugged in is not `Granted`.
fn granted_or_no_device(has_device: impl FnOnce() -> bool) -> PermState {
    if has_device() {
        PermState::Granted
    } else {
        PermState::NoDevice
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
        || {
            let granted = request_microphone_access();
            if granted {
                MICROPHONE_GRANT_OBSERVED.store(true, Ordering::Relaxed);
            }
            granted
        },
        has_microphone_device,
    )
}

/// The native prompt, and the only in-process event that reports its answer.
///
/// Going through the CoreAudio HAL instead (opening an input stream) raises
/// the same dialog but leaves AVFoundation's cached status untouched, so the
/// process keeps reading `NotDetermined` for the rest of its life.
#[cfg(target_os = "macos")]
fn request_microphone_access() -> bool {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeAudio};

    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    // A TCC request has to be raised from the main thread (SOU-122), and
    // `request` runs on a blocking pool thread. Only the call hops: it
    // returns as soon as the dialog is up, and the completion block fires
    // later on an arbitrary queue.
    let asked = on_main(move || unsafe {
        let Some(media_type) = AVMediaTypeAudio else {
            return false;
        };
        let block = RcBlock::new(move |granted: Bool| {
            let _ = tx.send(granted.as_bool());
        });
        AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &block);
        true
    });
    if !asked {
        return false;
    }
    // Generous: the user may leave the dialog on screen. The timeout only
    // bounds a callback that never comes.
    matches!(rx.recv_timeout(Duration::from_secs(300)), Ok(true))
}

#[cfg(not(target_os = "macos"))]
fn request_microphone_access() -> bool {
    false
}

// --- System audio (kTCCServiceAudioCapture) ---

/// `TCCAccessPreflight(service, NULL)`, from the private TCC framework.
/// CoreAudio ships no permission query for process taps, so this SPI is the
/// only way to read the status without mounting one. Unlike the public
/// preflight APIs it is an XPC round trip to tccd on every call, so it never
/// answers from a per-process cache.
#[cfg(all(target_os = "macos", feature = "private-tcc"))]
fn tcc_preflight(service: &str) -> Option<PermState> {
    use objc2_core_foundation::{CFRetained, CFString};
    use std::ffi::c_void;
    use std::sync::OnceLock;

    type TccAccessPreflight = unsafe extern "C" fn(*const c_void, *const c_void) -> i32;

    static SYMBOL: OnceLock<Option<TccAccessPreflight>> = OnceLock::new();
    let preflight = (*SYMBOL.get_or_init(|| unsafe {
        let handle = libc::dlopen(
            c"/System/Library/PrivateFrameworks/TCC.framework/Versions/A/TCC".as_ptr(),
            libc::RTLD_LAZY,
        );
        if handle.is_null() {
            tracing::warn!("TCC.framework not loadable; falling back to the tap probe");
            return None;
        }
        let symbol = libc::dlsym(handle, c"TCCAccessPreflight".as_ptr());
        if symbol.is_null() {
            tracing::warn!("TCCAccessPreflight missing; falling back to the tap probe");
            return None;
        }
        Some(std::mem::transmute::<*mut c_void, TccAccessPreflight>(
            symbol,
        ))
    }))?;

    let service = CFString::from_str(service);
    let raw = unsafe {
        preflight(
            CFRetained::as_ptr(&service).as_ptr().cast(),
            std::ptr::null(),
        )
    };
    tcc_preflight_state(raw)
}

#[cfg(not(all(target_os = "macos", feature = "private-tcc")))]
fn tcc_preflight(_service: &str) -> Option<PermState> {
    None
}

fn audio_capture_preflight() -> Option<PermState> {
    tcc_preflight("kTCCServiceAudioCapture")
}

/// AudioCap, Hyprnote and screenpipe all read the same three values out of
/// this SPI. Anything else means the contract moved, and an unknown number
/// must not be read as a verdict.
fn tcc_preflight_state(raw: i32) -> Option<PermState> {
    match raw {
        0 => Some(PermState::Granted),
        1 => Some(PermState::Denied),
        2 => Some(PermState::Unknown),
        other => {
            tracing::warn!(value = other, "Unexpected TCCAccessPreflight value");
            None
        }
    }
}

/// Mounts a tap for two seconds. This is the request path (the tap is what
/// raises the native prompt) and the read fallback when the TCC SPI is
/// unavailable. Never call it to render a status while the SPI answers.
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
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use crate::constants::APP_IDENTIFIER;
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

    /// The app never asks for Input Monitoring, but `CGEventTap::new` still
    /// makes macOS raise a ListenEvent access request for this process (a
    /// tap at the HID location asks for both listen and post rights). A TCC
    /// request with no usage string is refused out of hand, so the key stays.
    #[test]
    fn info_plist_declares_input_monitoring_usage() {
        let plist = std::fs::read_to_string(format!("{}/Info.plist", env!("CARGO_MANIFEST_DIR")))
            .expect("src-tauri/Info.plist");
        assert!(
            plist.contains("<key>NSInputMonitoringUsageDescription</key>"),
            "the native shortcut tap makes macOS ask for ListenEvent on our behalf"
        );
        assert!(
            plist.contains("keyboard and mouse events"),
            "usage string should describe what the tap reads"
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

    /// AC4: a settled `Denied` (or `Restricted`) opens Settings and must NOT
    /// ask again. macOS never re-prompts after an answer, so a request would
    /// return the same verdict without showing anything.
    #[test]
    fn denied_opens_settings_without_asking() {
        let opened = Cell::new(false);
        let asked = Cell::new(false);

        let result = request_microphone_with(
            || PermState::Denied,
            || opened.set(true),
            || {
                asked.set(true);
                true
            },
            || true,
        );

        assert_eq!(result, PermState::Denied);
        assert!(opened.get(), "Denied must open System Settings");
        assert!(!asked.get(), "Denied must not request access");
    }

    /// AC3: `NotDetermined` (modeled as `Unknown` here) goes through
    /// `requestAccess`, and the completion block's answer is the verdict.
    #[test]
    fn not_determined_requests_access_without_opening_settings() {
        let opened = Cell::new(false);
        let asked = Cell::new(false);

        let result = request_microphone_with(
            || PermState::Unknown,
            || opened.set(true),
            || {
                asked.set(true);
                true
            },
            || true,
        );

        assert_eq!(result, PermState::Granted);
        assert!(!opened.get(), "NotDetermined must not open Settings");
        assert!(asked.get(), "NotDetermined must request access");
    }

    #[test]
    fn a_refused_request_is_denied() {
        let result = request_microphone_with(
            || PermState::Unknown,
            || panic!("the request answers, so Settings must stay shut"),
            || false,
            || true,
        );
        assert_eq!(result, PermState::Denied);
    }

    /// A grant with nothing plugged in is not a permission problem, and the
    /// UI says something else about it.
    #[test]
    fn a_grant_without_an_input_device_is_no_device() {
        let result = request_microphone_with(|| PermState::Unknown, || {}, || true, || false);
        assert_eq!(result, PermState::NoDevice);
    }

    /// Already-authorized short-circuits to `Granted` without touching
    /// Settings or raising a prompt.
    #[test]
    fn granted_short_circuits() {
        let result = request_microphone_with(
            || PermState::Granted,
            || panic!("Granted must not open Settings"),
            || panic!("Granted must not request access"),
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
    fn microphone_snapshot_prefers_the_live_status_when_the_os_has_decided() {
        assert_eq!(
            microphone_snapshot_state(PermState::Denied, true),
            PermState::Denied,
            "a revoke read on this process must beat a grant seen earlier"
        );
        assert_eq!(
            microphone_snapshot_state(PermState::Granted, false),
            PermState::Granted
        );
    }

    /// AC5: the reported failure was a process that read NotDetermined two
    /// seconds before the user granted the permission and kept returning it
    /// from the AVFoundation cache, so the row showed "Grant" forever. A
    /// grant the request callback reported wins over that stale read.
    #[test]
    fn microphone_snapshot_uses_a_grant_this_process_observed() {
        assert_eq!(
            microphone_snapshot_state(PermState::Unknown, true),
            PermState::Granted
        );
        assert_eq!(
            microphone_snapshot_state(PermState::Unknown, false),
            PermState::Unknown
        );
    }

    /// AC6: the two permission rows are gone from the code, and a database
    /// still carrying them opens and reads back like any other.
    #[test]
    fn a_database_carrying_the_removed_permission_rows_still_loads() {
        let (db, dir) = crate::test_helpers::fixtures::test_db();
        db.set_setting("microphone_permission", "\"granted\"")
            .unwrap();
        db.set_setting("system_audio_permission", "\"denied\"")
            .unwrap();
        drop(db);

        let db = crate::db::Database::open(&dir.path().join("test.db"))
            .expect("a database holding the old permission rows must still open");
        let settings = db.get_all_settings().expect("settings must read back");
        assert!(
            settings
                .iter()
                .any(|(key, _)| key == "microphone_permission"),
            "the rows are left in place, just unread"
        );
    }

    #[test]
    fn system_audio_reads_the_tcc_preflight_without_mounting_a_tap() {
        for (raw, expected) in [
            (PermState::Granted, PermState::Granted),
            (PermState::Denied, PermState::Denied),
            (PermState::Unknown, PermState::Unknown),
        ] {
            let state = system_audio_state_with(
                true,
                || Some(raw),
                || panic!("a status read must not mount a tap when the SPI answered"),
            );
            assert_eq!(state, expected);
        }
    }

    /// AC8: a missing `TCCAccessPreflight` degrades to the tap probe. The
    /// arm is unreachable on a machine that has the symbol, so it is the
    /// injected `None` that keeps it honest.
    #[test]
    fn system_audio_falls_back_to_the_probe_when_the_spi_is_missing() {
        let probed = Cell::new(false);
        let state = system_audio_state_with(
            true,
            || None,
            || {
                probed.set(true);
                PermState::Granted
            },
        );
        assert_eq!(state, PermState::Granted);
        assert!(probed.get(), "a null dlsym must fall back, not panic");
    }

    #[test]
    fn system_audio_stays_unsupported_before_macos_14_4() {
        assert_eq!(
            system_audio_state_with(
                false,
                || panic!("an unsupported OS must not be asked"),
                || panic!("an unsupported OS must not be probed"),
            ),
            PermState::Unsupported
        );
    }

    /// 0/1/2 is the mapping AudioCap, Hyprnote and screenpipe agree on.
    #[test]
    fn tcc_preflight_values_map_to_states() {
        assert_eq!(tcc_preflight_state(0), Some(PermState::Granted));
        assert_eq!(tcc_preflight_state(1), Some(PermState::Denied));
        assert_eq!(tcc_preflight_state(2), Some(PermState::Unknown));
        assert_eq!(
            tcc_preflight_state(-1),
            None,
            "an unknown value must fall back, not be read as a verdict"
        );
    }
}
