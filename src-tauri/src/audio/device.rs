//! Stable CoreAudio input-device identity and preference helpers.

/// Human-facing transport label for an input device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TransportType {
    BuiltIn,
    Usb,
    Bluetooth,
    BluetoothLe,
    Virtual,
    Aggregate,
    Unknown,
}

/// An input-capable audio device as reported to the frontend.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct AudioInputDevice {
    /// Stable CoreAudio device UID (`kAudioDevicePropertyDeviceUID`).
    pub uid: String,
    pub name: String,
    pub transport: TransportType,
    pub is_default: bool,
}

/// Visible name of the private aggregate wrapping the process tap. This is
/// what CoreAudio (and the Settings device list) report — not the
/// `CATapDescription` name below. Copies leftover from a crash or a wedged
/// tap may appear as `"Souffle Tap 2"`, etc.
pub const SOUFFLE_TAP_AGGREGATE_NAME: &str = "Souffle Tap";

/// Name set on the `CATapDescription`. Kept for matching in case CoreAudio
/// ever surfaces the tap object itself rather than the aggregate.
pub const SOUFFLE_TAP_DESCRIPTION_NAME: &str = "Souffle system audio tap";

/// UID prefix for every aggregate we create, so orphans are identifiable
/// even if CoreAudio rewrites the display name.
pub const SOUFFLE_TAP_UID_PREFIX: &str = "com.souffle.tap.";

/// Whether `name` refers to Souffle's own system-audio tap aggregate (or a
/// numbered leftover like `"Souffle Tap 2"`).
pub fn is_souffle_tap_device(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("souffle") && lower.contains("tap")
}

/// Whether `uid` was issued by Souffle for a process-tap aggregate.
pub fn is_souffle_tap_uid(uid: &str) -> bool {
    uid.starts_with(SOUFFLE_TAP_UID_PREFIX)
}

/// Convert a stored preference (`uid` or legacy `name`) to the device name cpal
/// can match. Returns `None` when nothing in `devices` matches.
pub fn resolve_device_name<'a>(devices: &'a [AudioInputDevice], stored: &str) -> Option<&'a str> {
    devices
        .iter()
        .find(|device| device.uid == stored)
        .map(|device| device.name.as_str())
        .or_else(|| {
            devices
                .iter()
                .find(|device| device.name == stored)
                .map(|device| device.name.as_str())
        })
}

/// Map a cpal-opened device name to a stable UID from an enumeration snapshot.
/// Returns `None` when the name is absent (e.g. OS default opened before the
/// next CoreAudio list refresh); callers treat that as "needs reconcile".
pub fn uid_for_device_name(devices: &[AudioInputDevice], opened_name: &str) -> Option<String> {
    devices
        .iter()
        .find(|device| device.name == opened_name)
        .map(|device| device.uid.clone())
}

/// On upgrade, map a legacy name pin to the matching connected device's UID.
/// Returns `(value, changed)`.
pub fn migrate_stored_device_id(stored: &str, devices: &[AudioInputDevice]) -> (String, bool) {
    if devices.iter().any(|device| device.uid == stored) {
        return (stored.to_string(), false);
    }
    if let Some(device) = devices.iter().find(|device| device.name == stored) {
        return (device.uid.clone(), true);
    }
    (stored.to_string(), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(uid: &str, name: &str) -> AudioInputDevice {
        AudioInputDevice {
            uid: uid.into(),
            name: name.into(),
            transport: TransportType::BuiltIn,
            is_default: false,
        }
    }

    #[test]
    fn migrate_keeps_existing_uid() {
        let devices = vec![device("BuiltInMic", "MacBook Pro Microphone")];
        assert_eq!(
            migrate_stored_device_id("BuiltInMic", &devices),
            ("BuiltInMic".into(), false)
        );
    }

    #[test]
    fn migrate_maps_legacy_name_to_uid() {
        let devices = vec![device("UsbMicUid", "USB Microphone")];
        assert_eq!(
            migrate_stored_device_id("USB Microphone", &devices),
            ("UsbMicUid".into(), true)
        );
    }

    #[test]
    fn migrate_leaves_unknown_value_unchanged() {
        let devices = vec![device("BuiltInMic", "MacBook Pro Microphone")];
        assert_eq!(
            migrate_stored_device_id("Ghost Mic", &devices),
            ("Ghost Mic".into(), false)
        );
    }

    #[test]
    fn resolve_prefers_uid_match() {
        let devices = vec![
            device("uid-a", "Duplicate Name"),
            device("uid-b", "Duplicate Name"),
        ];
        assert_eq!(
            resolve_device_name(&devices, "uid-b"),
            Some("Duplicate Name")
        );
    }

    #[test]
    fn resolve_falls_back_to_legacy_name() {
        let devices = vec![device("uid-a", "USB Microphone")];
        assert_eq!(
            resolve_device_name(&devices, "USB Microphone"),
            Some("USB Microphone")
        );
    }

    #[test]
    fn resolve_returns_none_for_unknown_value() {
        let devices = vec![device("uid-a", "USB Microphone")];
        assert_eq!(resolve_device_name(&devices, "Missing"), None);
    }

    #[test]
    fn uid_for_device_name_matches_snapshot() {
        let devices = vec![
            device("uid-a", "Built-in Microphone"),
            device("uid-b", "USB Mic"),
        ];
        assert_eq!(
            uid_for_device_name(&devices, "USB Mic"),
            Some("uid-b".into()),
        );
    }

    #[test]
    fn uid_for_device_name_unknown_returns_none() {
        let devices = vec![device("uid-a", "Built-in Microphone")];
        assert_eq!(uid_for_device_name(&devices, "Ghost Mic"), None);
    }

    #[test]
    fn souffle_tap_name_is_detected() {
        assert!(is_souffle_tap_device("Souffle Tap"));
        assert!(is_souffle_tap_device("Souffle Tap 2"));
        assert!(is_souffle_tap_device("Souffle system audio tap"));
        assert!(!is_souffle_tap_device("MacBook Pro Microphone"));
        assert!(!is_souffle_tap_device("Souffle Mix"));
    }

    #[test]
    fn souffle_tap_uid_matches_prefix() {
        assert!(is_souffle_tap_uid(
            "com.souffle.tap.aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        ));
        assert!(!is_souffle_tap_uid("BuiltInMicUID"));
    }
}
