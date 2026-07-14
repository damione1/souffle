//! Stable CoreAudio input-device identity and preference helpers.

/// Human-facing transport label for an input device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct AudioInputDevice {
    /// Stable CoreAudio device UID (`kAudioDevicePropertyDeviceUID`).
    pub uid: String,
    pub name: String,
    pub transport: TransportType,
    pub is_default: bool,
}

/// Name of the private aggregate device created for system-audio capture.
pub const SOUFFLE_TAP_DEVICE_NAME: &str = "Souffle system audio tap";

/// Whether `name` refers to Souffle's own system-audio tap aggregate device.
pub fn is_souffle_tap_device(name: &str) -> bool {
    name.contains(SOUFFLE_TAP_DEVICE_NAME)
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
    fn souffle_tap_name_is_detected() {
        assert!(is_souffle_tap_device("Souffle system audio tap"));
        assert!(!is_souffle_tap_device("MacBook Pro Microphone"));
    }
}
