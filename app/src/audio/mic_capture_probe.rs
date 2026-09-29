//! One-shot "is a meeting app capturing the microphone right now?" probe.
//!
//! The calendar scheduler needs this answer at most once a minute, inside the
//! auto-start window of an event that has not been nudged yet. Reading it from
//! CoreAudio's per-process object list costs a few property reads and opens no
//! IO context, which matters: starting an IOProc (what a process tap needs)
//! makes coreaudiod raise `PreventUserIdleSystemSleep` on our behalf for as
//! long as the stream lives, so the previous streaming probe kept the machine
//! awake for the whole duration of every calendar event.
//!
//! `kAudioDevicePropertyDeviceIsRunningSomewhere` is deliberately not used
//! here: it reports any open stream on the device, including a silent one, and
//! says nothing about which app owns it.

/// Bundle ids treated as meeting / huddle clients, with their display label.
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

/// Human label for a known meeting bundle id, if any.
///
/// Matches the helper processes too: Chrome and the Electron apps open the
/// input from a child process whose bundle id extends the parent's
/// (`com.google.Chrome.helper`), and that child is the one CoreAudio lists as
/// running input.
pub fn meeting_app_label(bundle_id: &str) -> Option<&'static str> {
    KNOWN_MEETING_APPS
        .iter()
        .find(|(id, _)| bundle_id == *id || bundle_id.starts_with(&format!("{id}.")))
        .map(|(_, label)| *label)
}

/// What is holding the microphone open right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicCapture {
    /// A known meeting app, by display label.
    MeetingApp(&'static str),
    /// Something else has the default input device open. Browsers other than
    /// Chrome, and every conferencing tool not on the list, land here.
    OtherApp,
}

/// What is capturing the microphone right now, or `None` when nothing is.
///
/// Callers must not be recording themselves when they ask: Soufflé's own
/// capture would answer `OtherApp`.
pub fn mic_capture_in_progress() -> Option<MicCapture> {
    #[cfg(target_os = "macos")]
    {
        crate::platform::with_autorelease_pool(|| {
            // Per-process attribution is macOS 14.4+; the device-level answer
            // works everywhere and is the fallback when it is unavailable.
            if crate::platform::system_audio_capture_supported()
                && let Some(label) = macos::first_meeting_app_capturing_mic()
            {
                return Some(MicCapture::MeetingApp(label));
            }
            macos::default_input_is_running().then_some(MicCapture::OtherApp)
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ptr::NonNull;

    use objc2_app_kit::NSRunningApplication;
    use objc2_core_audio::{
        AudioObjectID, kAudioDevicePropertyDeviceIsRunningSomewhere,
        kAudioHardwarePropertyProcessObjectList, kAudioObjectSystemObject,
        kAudioProcessPropertyBundleID, kAudioProcessPropertyIsRunningInput,
        kAudioProcessPropertyPID,
    };
    use objc2_core_foundation::{CFRetained, CFString};

    use super::meeting_app_label;
    use crate::audio::device_watch::{audio_object_ids, get_property, global_address};

    pub(super) fn first_meeting_app_capturing_mic() -> Option<&'static str> {
        let system = kAudioObjectSystemObject as AudioObjectID;
        audio_object_ids(
            system,
            global_address(kAudioHardwarePropertyProcessObjectList),
        )
        .into_iter()
        .find_map(meeting_app_for_process)
    }

    fn meeting_app_for_process(process_id: AudioObjectID) -> Option<&'static str> {
        if !process_is_running_input(process_id) {
            return None;
        }
        let bundle_id = process_bundle_id(process_id).or_else(|| bundle_id_for_pid(process_id))?;
        meeting_app_label(&bundle_id)
    }

    /// Whether the default input device has a stream running, whoever owns
    /// it. Answers "someone is on the mic" for apps the allowlist misses.
    pub(super) fn default_input_is_running() -> bool {
        let Some(device_id) = crate::audio::device_watch::default_input_device_id() else {
            return false;
        };
        let mut running: u32 = 0;
        get_property(
            device_id,
            global_address(kAudioDevicePropertyDeviceIsRunningSomewhere),
            &mut running,
        ) && running != 0
    }

    fn process_is_running_input(process_id: AudioObjectID) -> bool {
        let mut running: u32 = 0;
        get_property(
            process_id,
            global_address(kAudioProcessPropertyIsRunningInput),
            &mut running,
        ) && running != 0
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
        NonNull::new(bundle_ptr.cast_mut())
            .map(|ptr| unsafe { CFRetained::from_raw(ptr) }.to_string())
    }

    /// Some processes expose no bundle id on the audio object; fall back to
    /// resolving the pid through AppKit.
    fn bundle_id_for_pid(process_id: AudioObjectID) -> Option<String> {
        let mut pid: i32 = 0;
        if !get_property(
            process_id,
            global_address(kAudioProcessPropertyPID),
            &mut pid,
        ) || pid <= 0
        {
            return None;
        }
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .and_then(|app| app.bundleIdentifier().map(|id| id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::{KNOWN_MEETING_APPS, meeting_app_label, mic_capture_in_progress};

    #[test]
    fn known_bundles_resolve_to_labels() {
        assert_eq!(meeting_app_label("us.zoom.xos"), Some("Zoom"));
        assert_eq!(meeting_app_label("com.apple.FaceTime"), Some("FaceTime"));
        assert_eq!(meeting_app_label("com.apple.Safari"), None);
    }

    /// The process CoreAudio reports as running input is usually a helper,
    /// not the app itself.
    #[test]
    fn helper_processes_resolve_to_their_parent_app() {
        assert_eq!(
            meeting_app_label("com.google.Chrome.helper"),
            Some("Google Chrome")
        );
        assert_eq!(
            meeting_app_label("com.tinyspeck.slackmacgap.helper.renderer"),
            Some("Slack")
        );
        // A different app that merely shares a prefix must not match.
        assert_eq!(meeting_app_label("com.google.ChromeCanary"), None);
    }

    #[test]
    fn known_meeting_apps_have_no_duplicate_bundle_ids() {
        let mut ids: Vec<&str> = KNOWN_MEETING_APPS.iter().map(|(id, _)| *id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(
            before,
            ids.len(),
            "duplicate bundle id in KNOWN_MEETING_APPS"
        );
    }

    /// The probe must be callable from any thread and never panic, whatever
    /// the machine is doing: the scheduler calls it on a blocking worker.
    #[test]
    fn probe_is_callable_and_total() {
        let _ = mic_capture_in_progress();
    }
}
