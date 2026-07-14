//! Meeting-app detection signals (groundwork; no start/stop policy yet).
//!
//! Observes known video-call apps via three macOS sources:
//! - CoreAudio per-process input capture (macOS 14.4+, ~1 s poll)
//! - `DeviceIsRunningSomewhere` on the default input device (macOS 13+ fallback)
//! - `NSWorkspace` launch/terminate notifications
//!
//! Emits typed begin/end signals and logs them. Recording start/stop policy is
//! handled elsewhere.

use std::collections::HashSet;
use std::fmt;

use tracing::info;

/// A known video-call or huddle app identified by bundle id.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MeetingAppRef {
    pub bundle_id: String,
    pub label: &'static str,
}

/// A meeting app currently capturing the microphone (per-process CoreAudio).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MicCapturingApp {
    pub pid: i32,
    pub bundle_id: String,
    pub label: &'static str,
}

/// Typed signals emitted when meeting-related state changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeetingDetectSignal {
    /// One or more known meeting apps started capturing the microphone.
    MicStarted(Vec<MicCapturingApp>),
    /// One or more known meeting apps stopped capturing the microphone.
    MicStopped(Vec<MicCapturingApp>),
    /// Something started using the default input device (fallback, no app id).
    MicCaptureActive,
    /// Nothing is capturing the default input device anymore (fallback).
    MicCaptureInactive,
    /// A known meeting app launched.
    MeetingAppLaunched(MeetingAppRef),
    /// A known meeting app terminated.
    MeetingAppTerminated(MeetingAppRef),
}

impl fmt::Display for MeetingDetectSignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MicStarted(apps) => write!(f, "mic started: {apps:?}"),
            Self::MicStopped(apps) => write!(f, "mic stopped: {apps:?}"),
            Self::MicCaptureActive => write!(f, "mic capture active"),
            Self::MicCaptureInactive => write!(f, "mic capture inactive"),
            Self::MeetingAppLaunched(app) => write!(f, "app launched: {} ({})", app.label, app.bundle_id),
            Self::MeetingAppTerminated(app) => write!(f, "app terminated: {} ({})", app.label, app.bundle_id),
        }
    }
}

/// Bundle ids for apps treated as meeting / huddle clients.
pub const KNOWN_MEETING_APPS: &[(&str, &str)] = &[
    ("us.zoom.xos", "Zoom"),
    ("com.microsoft.teams", "Microsoft Teams"),
    ("com.microsoft.teams2", "Microsoft Teams"),
    ("com.cisco.webexmeetingsapp", "Webex"),
    ("com.google.Chrome", "Google Chrome"),
    ("com.apple.FaceTime", "FaceTime"),
    ("com.hnc.Discord", "Discord"),
    ("com.tinyspeck.slackmacgap", "Slack"),
];

/// Whether `bundle_id` belongs to a known meeting app.
pub fn is_known_meeting_bundle(bundle_id: &str) -> bool {
    KNOWN_MEETING_APPS.iter().any(|(id, _)| *id == bundle_id)
}

/// Human label for a known meeting bundle id, if any.
pub fn meeting_app_label(bundle_id: &str) -> Option<&'static str> {
    KNOWN_MEETING_APPS
        .iter()
        .find(|(id, _)| *id == bundle_id)
        .map(|(_, label)| *label)
}

/// Resolve a bundle id to a [`MeetingAppRef`], or `None` when unknown.
pub fn meeting_app_ref(bundle_id: &str) -> Option<MeetingAppRef> {
    meeting_app_label(bundle_id).map(|label| MeetingAppRef {
        bundle_id: bundle_id.to_string(),
        label,
    })
}

pub struct MicAppDiff {
    pub started: Vec<MicCapturingApp>,
    pub stopped: Vec<MicCapturingApp>,
}

/// Diff two snapshots of meeting apps currently capturing the microphone.
pub fn diff_mic_apps(
    previous: &HashSet<MicCapturingApp>,
    current: &HashSet<MicCapturingApp>,
) -> MicAppDiff {
    let started = current.difference(previous).cloned().collect();
    let stopped = previous.difference(current).cloned().collect();
    MicAppDiff { started, stopped }
}

/// Keep only known meeting apps from a mic-capture snapshot.
pub fn filter_known_meeting_apps<I>(apps: I) -> HashSet<MicCapturingApp>
where
    I: IntoIterator<Item = MicCapturingApp>,
{
    apps.into_iter()
        .filter(|app| is_known_meeting_bundle(&app.bundle_id))
        .collect()
}

/// Result of applying a polled process snapshot to prior state.
pub struct ProcessSnapshotApply {
    pub snapshot: HashSet<MicCapturingApp>,
    pub primed: bool,
    pub signals: Vec<MeetingDetectSignal>,
}

/// Apply a process snapshot, seeding initial state without signals on the first
/// poll (same policy as [`mic_capture_transition`] for mic-capture fallback).
pub fn apply_process_snapshot(
    previous: &HashSet<MicCapturingApp>,
    current: HashSet<MicCapturingApp>,
    primed: bool,
) -> ProcessSnapshotApply {
    if !primed {
        return ProcessSnapshotApply {
            snapshot: current,
            primed: true,
            signals: Vec::new(),
        };
    }

    let diff = diff_mic_apps(previous, &current);
    let mut signals = Vec::new();
    if !diff.started.is_empty() {
        signals.push(MeetingDetectSignal::MicStarted(diff.started));
    }
    if !diff.stopped.is_empty() {
        signals.push(MeetingDetectSignal::MicStopped(diff.stopped));
    }

    ProcessSnapshotApply {
        snapshot: current,
        primed: true,
        signals,
    }
}

/// Whether mic-capture state transitioned; returns the new signal when it did.
pub fn mic_capture_transition(previous: Option<bool>, current: bool) -> Option<MeetingDetectSignal> {
    match previous {
        None => None,
        Some(prev) if prev == current => None,
        Some(true) => Some(MeetingDetectSignal::MicCaptureInactive),
        Some(false) => Some(MeetingDetectSignal::MicCaptureActive),
    }
}

/// Log a meeting-detect signal at info level.
pub fn log_signal(signal: &MeetingDetectSignal) {
    match signal {
        MeetingDetectSignal::MicStarted(apps) => {
            for app in apps {
                info!(
                    signal = "mic_started",
                    pid = app.pid,
                    bundle_id = %app.bundle_id,
                    label = app.label,
                    "Meeting detect signal"
                );
            }
        }
        MeetingDetectSignal::MicStopped(apps) => {
            for app in apps {
                info!(
                    signal = "mic_stopped",
                    pid = app.pid,
                    bundle_id = %app.bundle_id,
                    label = app.label,
                    "Meeting detect signal"
                );
            }
        }
        MeetingDetectSignal::MicCaptureActive => {
            info!(signal = "mic_capture_active", "Meeting detect signal");
        }
        MeetingDetectSignal::MicCaptureInactive => {
            info!(signal = "mic_capture_inactive", "Meeting detect signal");
        }
        MeetingDetectSignal::MeetingAppLaunched(app) => {
            info!(
                signal = "app_launched",
                bundle_id = %app.bundle_id,
                label = app.label,
                "Meeting detect signal"
            );
        }
        MeetingDetectSignal::MeetingAppTerminated(app) => {
            info!(
                signal = "app_terminated",
                bundle_id = %app.bundle_id,
                label = app.label,
                "Meeting detect signal"
            );
        }
    }
}

/// Handle keeping meeting-detect observers alive. Dropping it stops the poll loop.
#[cfg(target_os = "macos")]
pub struct MeetingDetectHandle {
    _stop_tx: std::sync::mpsc::Sender<()>,
    _mic_listeners: macos::MicCaptureListenerHandle,
}

#[cfg(not(target_os = "macos"))]
pub struct MeetingDetectHandle;

/// Start meeting-detect watchers. Must be called from the main thread (NSWorkspace).
/// Returns `None` on non-macOS targets.
#[cfg(target_os = "macos")]
pub fn start() -> Option<MeetingDetectHandle> {
    macos::start()
}

#[cfg(not(target_os = "macos"))]
pub fn start() -> Option<MeetingDetectHandle> {
    None
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::collections::HashSet;
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::sync::{Arc, Mutex, mpsc::Receiver};
    use std::thread;

    use block2::RcBlock;
    use dispatch2::{DispatchQueue, DispatchRetained};
    use objc2_app_kit::{
        NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
        NSWorkspaceDidLaunchApplicationNotification, NSWorkspaceDidTerminateApplicationNotification,
    };
    use objc2_core_audio::{
        AudioObjectAddPropertyListenerBlock, AudioObjectGetPropertyData,
        AudioObjectGetPropertyDataSize, AudioObjectID, AudioObjectPropertyAddress,
        kAudioDevicePropertyDeviceIsRunningSomewhere,
        kAudioHardwarePropertyDefaultInputDevice, kAudioHardwarePropertyProcessObjectList,
        kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal,
        kAudioObjectSystemObject, kAudioProcessPropertyBundleID, kAudioProcessPropertyIsRunningInput,
        kAudioProcessPropertyPID,
    };
    use objc2_core_foundation::{CFRetained, CFString};
    use objc2_foundation::NSNotification;
    use tracing::warn;

    enum InternalEvent {
        ProcessSnapshot(HashSet<MicCapturingApp>),
        MicCaptureRunning(bool),
        AppLaunched(MeetingAppRef),
        AppTerminated(MeetingAppRef),
    }

    pub(super) fn start() -> Option<MeetingDetectHandle> {
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();

        install_workspace_observers(event_tx.clone());
        let mic_listeners = install_mic_capture_listeners(event_tx.clone());

        if crate::platform::core_audio_process_detection_supported() {
            let poll_tx = event_tx;
            thread::Builder::new()
                .name("meeting-detect-poll".into())
                .spawn(move || run_process_poll_loop(poll_tx, stop_rx))
                .expect("failed to spawn meeting-detect poll thread");
        } else {
            info!("Meeting detect: per-process CoreAudio unavailable, using DeviceIsRunningSomewhere fallback only");
            drop(stop_rx);
        }

        thread::Builder::new()
            .name("meeting-detect".into())
            .spawn(move || run_signal_worker(event_rx))
            .expect("failed to spawn meeting-detect worker thread");

        Some(MeetingDetectHandle {
            _stop_tx: stop_tx,
            _mic_listeners: mic_listeners,
        })
    }

    type ListenerBlock = RcBlock<dyn Fn(u32, NonNull<AudioObjectPropertyAddress>)>;

    pub(super) struct MicCaptureListenerHandle {
        _queue: DispatchRetained<DispatchQueue>,
        #[allow(clippy::arc_with_non_send_sync)]
        _state: Arc<MicCaptureListenerState>,
    }

    struct MicCaptureListenerState {
        device_id: std::sync::atomic::AtomicU32,
        tx: std::sync::mpsc::Sender<InternalEvent>,
        queue: DispatchRetained<DispatchQueue>,
        blocks: Mutex<Vec<ListenerBlock>>,
    }

    fn run_signal_worker(rx: Receiver<InternalEvent>) {
        let mut mic_apps: HashSet<MicCapturingApp> = HashSet::new();
        let mut process_snapshot_primed = false;
        let mut mic_capture: Option<bool> = None;

        for event in rx {
            match event {
                InternalEvent::ProcessSnapshot(current) => {
                    let result =
                        apply_process_snapshot(&mic_apps, current, process_snapshot_primed);
                    mic_apps = result.snapshot;
                    process_snapshot_primed = result.primed;
                    for signal in result.signals {
                        log_signal(&signal);
                    }
                }
                InternalEvent::MicCaptureRunning(running) => {
                    if let Some(signal) = mic_capture_transition(mic_capture, running) {
                        log_signal(&signal);
                    }
                    mic_capture = Some(running);
                }
                InternalEvent::AppLaunched(app) => {
                    let signal = MeetingDetectSignal::MeetingAppLaunched(app);
                    log_signal(&signal);
                }
                InternalEvent::AppTerminated(app) => {
                    let signal = MeetingDetectSignal::MeetingAppTerminated(app);
                    log_signal(&signal);
                }
            }
        }
    }

    fn run_process_poll_loop(
        tx: std::sync::mpsc::Sender<InternalEvent>,
        stop_rx: Receiver<()>,
    ) {
        use std::time::Duration;

        loop {
            if poll_stop_requested(&stop_rx) {
                break;
            }
            let snapshot = snapshot_meeting_mic_apps();
            let _ = tx.send(InternalEvent::ProcessSnapshot(snapshot));
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    fn poll_stop_requested(stop_rx: &Receiver<()>) -> bool {
        match stop_rx.try_recv() {
            Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => true,
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
        }
    }

    fn install_workspace_observers(tx: std::sync::mpsc::Sender<InternalEvent>) {
        let center = NSWorkspace::sharedWorkspace().notificationCenter();

        let launch_tx = tx.clone();
        let launch_block = RcBlock::new(move |note: NonNull<NSNotification>| {
            let note = unsafe { note.as_ref() };
            if let Some(app) = meeting_app_from_notification(note) {
                let _ = launch_tx.send(InternalEvent::AppLaunched(app));
            }
        });
        let terminate_tx = tx;
        let terminate_block = RcBlock::new(move |note: NonNull<NSNotification>| {
            let note = unsafe { note.as_ref() };
            if let Some(app) = meeting_app_from_notification(note) {
                let _ = terminate_tx.send(InternalEvent::AppTerminated(app));
            }
        });

        unsafe {
            let launch_token = center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidLaunchApplicationNotification),
                None,
                None,
                &launch_block,
            );
            std::mem::forget(launch_token);

            let terminate_token = center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidTerminateApplicationNotification),
                None,
                None,
                &terminate_block,
            );
            std::mem::forget(terminate_token);
        }
    }

    fn meeting_app_from_notification(note: &NSNotification) -> Option<MeetingAppRef> {
        let info = note.userInfo()?;
        let key = unsafe { NSWorkspaceApplicationKey.as_ref() };
        let running = info.objectForKey(key)?;
        let running = running.downcast::<NSRunningApplication>().ok()?;
        let bundle_id = running.bundleIdentifier()?.to_string();
        meeting_app_ref(&bundle_id)
    }

    #[allow(clippy::arc_with_non_send_sync)]
    fn install_mic_capture_listeners(
        tx: std::sync::mpsc::Sender<InternalEvent>,
    ) -> MicCaptureListenerHandle {
        let queue = DispatchQueue::new("com.souffle.meeting-detect-mic", None);
        let state = Arc::new(MicCaptureListenerState {
            device_id: std::sync::atomic::AtomicU32::new(0),
            tx: tx.clone(),
            queue: queue.clone(),
            blocks: Mutex::new(Vec::new()),
        });

        let default_block = add_default_input_listener(Arc::clone(&state));
        state.blocks.lock().expect("mic listener blocks lock").push(default_block);

        if let Some(device_id) = default_input_device_id() {
            state.device_id.store(device_id, std::sync::atomic::Ordering::Release);
            attach_device_listener(device_id, Arc::clone(&state));
        }

        MicCaptureListenerHandle {
            _queue: queue,
            _state: state,
        }
    }

    fn add_default_input_listener(state: Arc<MicCaptureListenerState>) -> ListenerBlock {
        let queue = state.queue.clone();
        let callback_state = Arc::clone(&state);
        let block: ListenerBlock = RcBlock::new(
            move |_count: u32, _addresses: NonNull<AudioObjectPropertyAddress>| {
                on_default_input_changed(Arc::clone(&callback_state));
            },
        );

        let mut address = global_address(kAudioHardwarePropertyDefaultInputDevice);
        let status = unsafe {
            AudioObjectAddPropertyListenerBlock(
                kAudioObjectSystemObject as AudioObjectID,
                NonNull::from(&mut address),
                Some(&queue),
                RcBlock::as_ptr(&block),
            )
        };
        if status != 0 {
            warn!(
                status,
                "Meeting detect: failed to register default input device listener"
            );
        }
        block
    }

    fn on_default_input_changed(state: Arc<MicCaptureListenerState>) {
        let Some(device_id) = default_input_device_id() else {
            return;
        };
        let previous = state
            .device_id
            .swap(device_id, std::sync::atomic::Ordering::AcqRel);
        if previous != device_id {
            attach_device_listener(device_id, Arc::clone(&state));
        }
        let running = is_mic_running_somewhere(device_id);
        let _ = state.tx.send(InternalEvent::MicCaptureRunning(running));
    }

    fn attach_device_listener(device_id: AudioObjectID, state: Arc<MicCaptureListenerState>) {
        let queue = state.queue.clone();
        let callback_state = Arc::clone(&state);
        let block: ListenerBlock = RcBlock::new(
            move |_count: u32, _addresses: NonNull<AudioObjectPropertyAddress>| {
                let current = callback_state
                    .device_id
                    .load(std::sync::atomic::Ordering::Acquire);
                if current == 0 || current != device_id {
                    return;
                }
                let running = is_mic_running_somewhere(device_id);
                let _ = callback_state
                    .tx
                    .send(InternalEvent::MicCaptureRunning(running));
            },
        );

        let mut address = global_address(kAudioDevicePropertyDeviceIsRunningSomewhere);
        let status = unsafe {
            AudioObjectAddPropertyListenerBlock(
                device_id,
                NonNull::from(&mut address),
                Some(&queue),
                RcBlock::as_ptr(&block),
            )
        };
        if status != 0 {
            warn!(
                device_id,
                status,
                "Meeting detect: failed to register DeviceIsRunningSomewhere listener"
            );
        } else {
            state.blocks.lock().expect("mic listener blocks lock").push(block);
        }
    }

    fn snapshot_meeting_mic_apps() -> HashSet<MicCapturingApp> {
        let system = kAudioObjectSystemObject as AudioObjectID;
        let process_ids = device_ids(system, global_address(kAudioHardwarePropertyProcessObjectList));
        process_ids
            .into_iter()
            .filter_map(mic_capturing_app)
            .collect()
    }

    fn mic_capturing_app(process_id: AudioObjectID) -> Option<MicCapturingApp> {
        if !process_is_running_input(process_id) {
            return None;
        }
        let pid = process_pid(process_id)?;
        let bundle_id = process_bundle_id(process_id).or_else(|| bundle_id_for_pid(pid))?;
        let label = meeting_app_label(&bundle_id)?;
        Some(MicCapturingApp {
            pid,
            bundle_id,
            label,
        })
    }

    fn process_is_running_input(process_id: AudioObjectID) -> bool {
        let mut running: u32 = 0;
        get_property(
            process_id,
            global_address(kAudioProcessPropertyIsRunningInput),
            &mut running,
        ) && running != 0
    }

    fn process_pid(process_id: AudioObjectID) -> Option<i32> {
        let mut pid: i32 = 0;
        get_property(process_id, global_address(kAudioProcessPropertyPID), &mut pid)
            .then_some(pid)
            .filter(|&pid| pid > 0)
    }

    fn process_bundle_id(process_id: AudioObjectID) -> Option<String> {
        let mut bundle_ptr: *const CFString = std::ptr::null();
        if !get_property(
            process_id,
            global_address(kAudioProcessPropertyBundleID),
            &mut bundle_ptr,
        ) {
            return None;
        }
        NonNull::new(bundle_ptr.cast_mut()).map(|ptr| unsafe { CFRetained::from_raw(ptr) }.to_string())
    }

    fn bundle_id_for_pid(pid: i32) -> Option<String> {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .and_then(|app| app.bundleIdentifier().map(|s| s.to_string()))
    }

    fn is_mic_running_somewhere(device_id: AudioObjectID) -> bool {
        let mut running: u32 = 0;
        get_property(
            device_id,
            global_address(kAudioDevicePropertyDeviceIsRunningSomewhere),
            &mut running,
        ) && running != 0
    }

    fn default_input_device_id() -> Option<AudioObjectID> {
        let mut device: AudioObjectID = 0;
        get_property(
            kAudioObjectSystemObject as AudioObjectID,
            global_address(kAudioHardwarePropertyDefaultInputDevice),
            &mut device,
        )
        .then_some(device)
        .filter(|&id| id != 0)
    }

    fn global_address(selector: u32) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress {
            mSelector: selector,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        }
    }

    fn get_property<T>(object: AudioObjectID, mut address: AudioObjectPropertyAddress, out: &mut T) -> bool {
        let mut size = size_of::<T>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                NonNull::from(&mut address),
                0,
                std::ptr::null(),
                NonNull::from(&mut size),
                NonNull::new(out as *mut T as *mut c_void).expect("non-null out pointer"),
            )
        };
        status == 0
    }

    fn property_data_size(object: AudioObjectID, mut address: AudioObjectPropertyAddress) -> u32 {
        let mut size: u32 = 0;
        let status = unsafe {
            AudioObjectGetPropertyDataSize(
                object,
                NonNull::from(&mut address),
                0,
                std::ptr::null(),
                NonNull::from(&mut size),
            )
        };
        if status == 0 { size } else { 0 }
    }

    fn device_ids(object: AudioObjectID, address: AudioObjectPropertyAddress) -> Vec<AudioObjectID> {
        let size = property_data_size(object, address);
        let count = size as usize / size_of::<AudioObjectID>();
        if count == 0 {
            return Vec::new();
        }
        let mut ids = vec![0 as AudioObjectID; count];
        let mut out_size = size;
        let mut addr = address;
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                NonNull::from(&mut addr),
                0,
                std::ptr::null(),
                NonNull::from(&mut out_size),
                NonNull::new(ids.as_mut_ptr() as *mut c_void).expect("non-null out pointer"),
            )
        };
        if status != 0 {
            return Vec::new();
        }
        ids.truncate(out_size as usize / size_of::<AudioObjectID>());
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mic_app(pid: i32, bundle_id: &str) -> MicCapturingApp {
        MicCapturingApp {
            pid,
            bundle_id: bundle_id.to_string(),
            label: meeting_app_label(bundle_id).unwrap_or("Unknown"),
        }
    }

    #[test]
    fn known_meeting_bundle_ids() {
        assert!(is_known_meeting_bundle("us.zoom.xos"));
        assert!(is_known_meeting_bundle("com.microsoft.teams2"));
        assert!(is_known_meeting_bundle("com.google.Chrome"));
        assert!(!is_known_meeting_bundle("com.apple.Safari"));
    }

    #[test]
    fn meeting_app_ref_unknown_is_none() {
        assert!(meeting_app_ref("com.example.app").is_none());
        let app = meeting_app_ref("us.zoom.xos").expect("zoom");
        assert_eq!(app.label, "Zoom");
    }

    #[test]
    fn diff_mic_apps_detects_start() {
        let previous = HashSet::new();
        let mut current = HashSet::new();
        current.insert(mic_app(100, "us.zoom.xos"));

        let diff = diff_mic_apps(&previous, &current);

        assert_eq!(diff.started.len(), 1);
        assert_eq!(diff.started[0].pid, 100);
        assert!(diff.stopped.is_empty());
    }

    #[test]
    fn diff_mic_apps_detects_stop() {
        let mut previous = HashSet::new();
        previous.insert(mic_app(100, "us.zoom.xos"));
        let current = HashSet::new();

        let diff = diff_mic_apps(&previous, &current);

        assert!(diff.started.is_empty());
        assert_eq!(diff.stopped.len(), 1);
        assert_eq!(diff.stopped[0].bundle_id, "us.zoom.xos");
    }

    #[test]
    fn diff_mic_apps_ignores_unchanged() {
        let mut set = HashSet::new();
        set.insert(mic_app(100, "us.zoom.xos"));

        let diff = diff_mic_apps(&set, &set);

        assert!(diff.started.is_empty());
        assert!(diff.stopped.is_empty());
    }

    #[test]
    fn diff_mic_apps_detects_swap() {
        let mut previous = HashSet::new();
        previous.insert(mic_app(100, "us.zoom.xos"));
        let mut current = HashSet::new();
        current.insert(mic_app(200, "com.microsoft.teams2"));

        let diff = diff_mic_apps(&previous, &current);

        assert_eq!(diff.started.len(), 1);
        assert_eq!(diff.stopped.len(), 1);
        assert_eq!(diff.started[0].pid, 200);
        assert_eq!(diff.stopped[0].pid, 100);
    }

    #[test]
    fn filter_known_meeting_apps_drops_unknown() {
        let apps = vec![
            mic_app(1, "us.zoom.xos"),
            mic_app(2, "com.apple.Safari"),
        ];
        let filtered = filter_known_meeting_apps(apps);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered.iter().next().unwrap().bundle_id, "us.zoom.xos");
    }

    #[test]
    fn apply_process_snapshot_first_poll_seeds_without_signals() {
        let previous = HashSet::new();
        let mut current = HashSet::new();
        current.insert(mic_app(100, "us.zoom.xos"));

        let result = apply_process_snapshot(&previous, current.clone(), false);

        assert!(result.signals.is_empty());
        assert!(result.primed);
        assert_eq!(result.snapshot, current);
    }

    #[test]
    fn apply_process_snapshot_after_primed_emits_start() {
        let mut previous = HashSet::new();
        previous.insert(mic_app(100, "us.zoom.xos"));
        let mut current = previous.clone();
        current.insert(mic_app(200, "com.microsoft.teams2"));

        let result = apply_process_snapshot(&previous, current.clone(), true);

        assert_eq!(result.snapshot, current);
        assert_eq!(result.signals.len(), 1);
        assert_eq!(
            result.signals[0],
            MeetingDetectSignal::MicStarted(vec![mic_app(200, "com.microsoft.teams2")])
        );
    }

    #[test]
    fn apply_process_snapshot_after_primed_emits_stop() {
        let mut previous = HashSet::new();
        previous.insert(mic_app(100, "us.zoom.xos"));
        let current = HashSet::new();

        let result = apply_process_snapshot(&previous, current.clone(), true);

        assert_eq!(result.snapshot, current);
        assert_eq!(result.signals.len(), 1);
        assert_eq!(
            result.signals[0],
            MeetingDetectSignal::MicStopped(vec![mic_app(100, "us.zoom.xos")])
        );
    }

    #[test]
    fn mic_capture_transition_active() {
        let signal = mic_capture_transition(Some(false), true).expect("transition");
        assert_eq!(signal, MeetingDetectSignal::MicCaptureActive);
    }

    #[test]
    fn mic_capture_transition_inactive() {
        let signal = mic_capture_transition(Some(true), false).expect("transition");
        assert_eq!(signal, MeetingDetectSignal::MicCaptureInactive);
    }

    #[test]
    fn mic_capture_transition_initial_state_is_silent() {
        assert!(mic_capture_transition(None, true).is_none());
        assert!(mic_capture_transition(None, false).is_none());
    }

    #[test]
    fn mic_capture_transition_no_change() {
        assert!(mic_capture_transition(Some(true), true).is_none());
        assert!(mic_capture_transition(Some(false), false).is_none());
    }

    #[test]
    fn signal_display_is_human_readable() {
        let signal = MeetingDetectSignal::MeetingAppLaunched(MeetingAppRef {
            bundle_id: "us.zoom.xos".into(),
            label: "Zoom",
        });
        assert!(signal.to_string().contains("Zoom"));
    }
}
