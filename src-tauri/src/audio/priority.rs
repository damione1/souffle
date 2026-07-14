//! Input-device priority policy and resolution.

use std::time::{SystemTime, UNIX_EPOCH};

use super::device::{AudioInputDevice, TransportType};

/// A device remembered across disconnects for display and preference ordering.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct KnownDevice {
    pub uid: String,
    pub name: String,
    /// Unix timestamp (seconds) when this device was last seen connected.
    pub last_seen: i64,
}

/// User-declared input routing preferences (UID-based).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct InputPriority {
    /// Preferred device UIDs, highest priority first.
    pub priorities: Vec<String>,
    /// Connected devices in this list are never auto-selected.
    pub hidden: Vec<String>,
    /// Devices seen before, including ones currently disconnected.
    pub known: Vec<KnownDevice>,
}

/// Parameters for [`resolve_input`]; kept separate so resolution stays pure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolveInputParams<'a> {
    /// Explicit user pin (settings `audio_device`), if any.
    pub pin: Option<&'a str>,
    /// Clamshell-mode preference UID, if configured.
    pub clamshell_pref: Option<&'a str>,
    /// Whether clamshell mode is active right now.
    pub clamshell_active: bool,
    pub priority: &'a InputPriority,
    /// When false, Bluetooth inputs are skipped unless explicitly pinned or
    /// configured for clamshell (avoids waking HFP mono on headset output).
    pub allow_bluetooth_mic: bool,
}

/// Whether `transport` is a Bluetooth headset input.
pub fn is_bluetooth_transport(transport: TransportType) -> bool {
    matches!(
        transport,
        TransportType::Bluetooth | TransportType::BluetoothLe
    )
}

fn is_connected<'a>(devices: &'a [AudioInputDevice], uid: &str) -> Option<&'a AudioInputDevice> {
    devices.iter().find(|device| device.uid == uid)
}

fn is_auto_eligible(device: &AudioInputDevice, hidden: &[String], allow_bluetooth_mic: bool) -> bool {
    !hidden.iter().any(|uid| uid == &device.uid)
        && (allow_bluetooth_mic || !is_bluetooth_transport(device.transport))
}

/// Update `known` from the current connected-device snapshot.
pub fn touch_known(priority: &mut InputPriority, connected: &[AudioInputDevice]) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);

    for device in connected {
        if let Some(known) = priority.known.iter_mut().find(|entry| entry.uid == device.uid) {
            known.name = device.name.clone();
            known.last_seen = now;
        } else {
            priority.known.push(KnownDevice {
                uid: device.uid.clone(),
                name: device.name.clone(),
                last_seen: now,
            });
        }
    }
}

/// Pick the input UID to capture on, or `None` when no device is connected.
///
/// Order: explicit pin (if connected) -> clamshell preference (when active and
/// connected) -> first connected non-hidden priority entry -> system default
/// (with anti-Bluetooth preference when `allow_bluetooth_mic` is false).
///
/// `None` is also returned when devices are connected but none is
/// auto-eligible (all hidden, or Bluetooth-only with Bluetooth disallowed).
/// The caller decides that fallback: capture opens the OS default anyway so
/// recording still works, and never rebuilds toward a `None` resolution.
pub fn resolve_input(connected: &[AudioInputDevice], params: ResolveInputParams<'_>) -> Option<String> {
    if connected.is_empty() {
        return None;
    }

    if let Some(pin) = params.pin.filter(|uid| is_connected(connected, uid).is_some()) {
        return Some(pin.to_string());
    }

    if params.clamshell_active
        && let Some(uid) = params
            .clamshell_pref
            .filter(|uid| is_connected(connected, uid).is_some())
    {
        return Some(uid.to_string());
    }

    for uid in &params.priority.priorities {
        if let Some(device) = is_connected(connected, uid)
            && is_auto_eligible(device, &params.priority.hidden, params.allow_bluetooth_mic)
        {
            return Some(device.uid.clone());
        }
    }

    resolve_default(connected, &params.priority.hidden, params.allow_bluetooth_mic)
}

fn resolve_default(
    connected: &[AudioInputDevice],
    hidden: &[String],
    allow_bluetooth_mic: bool,
) -> Option<String> {
    let eligible: Vec<&AudioInputDevice> = connected
        .iter()
        .filter(|device| is_auto_eligible(device, hidden, allow_bluetooth_mic))
        .collect();

    if eligible.is_empty() {
        return None;
    }

    if let Some(device) = eligible.iter().find(|device| device.is_default) {
        return Some(device.uid.clone());
    }

    for preferred in [TransportType::BuiltIn, TransportType::Usb] {
        if let Some(device) = eligible.iter().find(|device| device.transport == preferred) {
            return Some(device.uid.clone());
        }
    }

    Some(eligible[0].uid.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(uid: &str, name: &str, transport: TransportType, is_default: bool) -> AudioInputDevice {
        AudioInputDevice {
            uid: uid.into(),
            name: name.into(),
            transport,
            is_default,
        }
    }

    fn empty_priority() -> InputPriority {
        InputPriority::default()
    }

    fn params<'a>(
        pin: Option<&'a str>,
        clamshell_pref: Option<&'a str>,
        clamshell_active: bool,
        priority: &'a InputPriority,
        allow_bluetooth_mic: bool,
    ) -> ResolveInputParams<'a> {
        ResolveInputParams {
            pin,
            clamshell_pref,
            clamshell_active,
            priority,
            allow_bluetooth_mic,
        }
    }

    #[test]
    fn pin_wins_when_connected() {
        let connected = vec![
            device("builtin", "Built-in", TransportType::BuiltIn, true),
            device("usb", "USB", TransportType::Usb, false),
        ];
        assert_eq!(
            resolve_input(
                &connected,
                params(Some("usb"), None, false, &empty_priority(), false),
            ),
            Some("usb".into()),
        );
    }

    #[test]
    fn disconnected_pin_falls_through() {
        let connected = vec![device("builtin", "Built-in", TransportType::BuiltIn, true)];
        assert_eq!(
            resolve_input(
                &connected,
                params(Some("ghost"), None, false, &empty_priority(), false),
            ),
            Some("builtin".into()),
        );
    }

    #[test]
    fn clamshell_applies_without_pin() {
        let connected = vec![
            device("builtin", "Built-in", TransportType::BuiltIn, true),
            device("webcam", "Webcam", TransportType::Usb, false),
        ];
        assert_eq!(
            resolve_input(
                &connected,
                params(None, Some("webcam"), true, &empty_priority(), false),
            ),
            Some("webcam".into()),
        );
    }

    #[test]
    fn pin_wins_over_clamshell() {
        let connected = vec![
            device("builtin", "Built-in", TransportType::BuiltIn, true),
            device("webcam", "Webcam", TransportType::Usb, false),
        ];
        assert_eq!(
            resolve_input(
                &connected,
                params(Some("builtin"), Some("webcam"), true, &empty_priority(), false),
            ),
            Some("builtin".into()),
        );
    }

    #[test]
    fn clamshell_ignored_when_not_active() {
        let connected = vec![
            device("builtin", "Built-in", TransportType::BuiltIn, true),
            device("webcam", "Webcam", TransportType::Usb, false),
        ];
        assert_eq!(
            resolve_input(
                &connected,
                params(None, Some("webcam"), false, &empty_priority(), false),
            ),
            Some("builtin".into()),
        );
    }

    #[test]
    fn priorities_pick_first_connected_eligible() {
        let connected = vec![
            device("builtin", "Built-in", TransportType::BuiltIn, true),
            device("usb", "USB", TransportType::Usb, false),
            device("hdmi", "HDMI", TransportType::Unknown, false),
        ];
        let priority = InputPriority {
            priorities: vec!["missing".into(), "hdmi".into(), "usb".into()],
            hidden: Vec::new(),
            known: Vec::new(),
        };
        assert_eq!(
            resolve_input(&connected, params(None, None, false, &priority, true)),
            Some("hdmi".into()),
        );
    }

    #[test]
    fn hidden_devices_are_skipped_in_priorities() {
        let connected = vec![
            device("usb", "USB", TransportType::Usb, false),
            device("builtin", "Built-in", TransportType::BuiltIn, true),
        ];
        let priority = InputPriority {
            priorities: vec!["usb".into(), "builtin".into()],
            hidden: vec!["usb".into()],
            known: Vec::new(),
        };
        assert_eq!(
            resolve_input(&connected, params(None, None, false, &priority, true)),
            Some("builtin".into()),
        );
    }

    #[test]
    fn bluetooth_default_skipped_when_not_allowed() {
        let connected = vec![
            device("bt", "AirPods", TransportType::Bluetooth, true),
            device("builtin", "Built-in", TransportType::BuiltIn, false),
        ];
        assert_eq!(
            resolve_input(
                &connected,
                params(None, None, false, &empty_priority(), false),
            ),
            Some("builtin".into()),
        );
    }

    #[test]
    fn bluetooth_default_used_when_allowed() {
        let connected = vec![
            device("bt", "AirPods", TransportType::Bluetooth, true),
            device("builtin", "Built-in", TransportType::BuiltIn, false),
        ];
        assert_eq!(
            resolve_input(
                &connected,
                params(None, None, false, &empty_priority(), true),
            ),
            Some("bt".into()),
        );
    }

    #[test]
    fn pinned_bluetooth_still_used_when_not_allowed() {
        let connected = vec![
            device("bt", "AirPods", TransportType::Bluetooth, true),
            device("builtin", "Built-in", TransportType::BuiltIn, false),
        ];
        assert_eq!(
            resolve_input(
                &connected,
                params(Some("bt"), None, false, &empty_priority(), false),
            ),
            Some("bt".into()),
        );
    }

    #[test]
    fn bluetooth_skipped_in_priorities_when_not_allowed() {
        let connected = vec![
            device("bt", "AirPods", TransportType::Bluetooth, false),
            device("usb", "USB", TransportType::Usb, true),
        ];
        let priority = InputPriority {
            priorities: vec!["bt".into(), "usb".into()],
            hidden: Vec::new(),
            known: Vec::new(),
        };
        assert_eq!(
            resolve_input(&connected, params(None, None, false, &priority, false)),
            Some("usb".into()),
        );
    }

    #[test]
    fn empty_connected_returns_none() {
        assert_eq!(
            resolve_input(&[], params(None, None, false, &empty_priority(), false)),
            None,
        );
    }

    #[test]
    fn bluetooth_only_returns_none_for_caller_fallback() {
        let connected = vec![device("bt", "AirPods", TransportType::Bluetooth, true)];
        assert_eq!(
            resolve_input(
                &connected,
                params(None, None, false, &empty_priority(), false),
            ),
            None,
        );
    }

    #[test]
    fn touch_known_updates_and_appends() {
        let mut priority = InputPriority {
            known: vec![KnownDevice {
                uid: "builtin".into(),
                name: "Old name".into(),
                last_seen: 1,
            }],
            ..InputPriority::default()
        };
        let connected = vec![
            device("builtin", "Built-in", TransportType::BuiltIn, true),
            device("usb", "USB", TransportType::Usb, false),
        ];
        touch_known(&mut priority, &connected);
        assert_eq!(priority.known.len(), 2);
        assert_eq!(priority.known[0].name, "Built-in");
        assert!(priority.known[0].last_seen > 1);
        assert_eq!(priority.known[1].uid, "usb");
    }
}
