//! Input-device priority policy and resolution.

use std::time::{SystemTime, UNIX_EPOCH};

use super::device::{AudioInputDevice, TransportType};

/// A device remembered across disconnects for display and preference ordering.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KnownDevice {
    pub uid: String,
    pub name: String,
    /// Unix timestamp (seconds) when this device was last seen connected.
    pub last_seen: i64,
}

/// User-declared input routing preferences (UID-based).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct InputPriority {
    /// Preferred device UIDs, highest priority first.
    pub priorities: Vec<String>,
    /// Devices seen before, including ones currently disconnected.
    pub known: Vec<KnownDevice>,
}

impl InputPriority {
    /// Trim every entry, then drop blank and repeated ones.
    pub fn sanitize(&mut self) {
        self.priorities = self
            .priorities
            .iter()
            .map(|uid| uid.trim().to_string())
            .filter(|uid| !uid.is_empty())
            .collect();
        self.priorities.dedup();
        self.known = self
            .known
            .iter()
            .filter_map(|entry| {
                let uid = entry.uid.trim();
                let name = entry.name.trim();
                (!uid.is_empty() && !name.is_empty()).then(|| KnownDevice {
                    uid: uid.to_string(),
                    name: name.to_string(),
                    last_seen: entry.last_seen,
                })
            })
            .collect();
        let mut seen = std::collections::HashSet::new();
        self.known.retain(|entry| seen.insert(entry.uid.clone()));
    }

    /// Move `uid` to the top of the preference order.
    pub fn promote(&mut self, uid: &str) {
        self.priorities.retain(|entry| entry != uid);
        self.priorities.insert(0, uid.to_string());
    }
}

/// Parameters for [`resolve_input`]; kept separate so resolution stays pure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolveInputParams<'a> {
    /// Clamshell-mode preference UID, if configured.
    pub clamshell_pref: Option<&'a str>,
    /// Whether clamshell mode is active right now.
    pub clamshell_active: bool,
    pub priority: &'a InputPriority,
}

fn is_connected<'a>(devices: &'a [AudioInputDevice], uid: &str) -> Option<&'a AudioInputDevice> {
    devices.iter().find(|device| device.uid == uid)
}

/// Update `known` from the current connected-device snapshot.
pub fn touch_known(priority: &mut InputPriority, connected: &[AudioInputDevice]) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);

    for device in connected {
        if let Some(known) = priority
            .known
            .iter_mut()
            .find(|entry| entry.uid == device.uid)
        {
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
/// Order: clamshell preference (when active and connected) -> first connected
/// priority entry -> system default.
pub fn resolve_input(
    connected: &[AudioInputDevice],
    params: ResolveInputParams<'_>,
) -> Option<String> {
    if connected.is_empty() {
        return None;
    }

    if params.clamshell_active
        && let Some(uid) = params
            .clamshell_pref
            .filter(|uid| is_connected(connected, uid).is_some())
    {
        return Some(uid.to_string());
    }

    for uid in &params.priority.priorities {
        if let Some(device) = is_connected(connected, uid) {
            return Some(device.uid.clone());
        }
    }

    resolve_default(connected)
}

fn resolve_default(connected: &[AudioInputDevice]) -> Option<String> {
    if let Some(device) = connected.iter().find(|device| device.is_default) {
        return Some(device.uid.clone());
    }

    for preferred in [TransportType::BuiltIn, TransportType::Usb] {
        if let Some(device) = connected
            .iter()
            .find(|device| device.transport == preferred)
        {
            return Some(device.uid.clone());
        }
    }

    connected.first().map(|device| device.uid.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(
        uid: &str,
        name: &str,
        transport: TransportType,
        is_default: bool,
    ) -> AudioInputDevice {
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
        clamshell_pref: Option<&'a str>,
        clamshell_active: bool,
        priority: &'a InputPriority,
    ) -> ResolveInputParams<'a> {
        ResolveInputParams {
            clamshell_pref,
            clamshell_active,
            priority,
        }
    }

    #[test]
    fn clamshell_wins_over_the_order_when_active() {
        let connected = vec![
            device("builtin", "Built-in", TransportType::BuiltIn, true),
            device("webcam", "Webcam", TransportType::Usb, false),
        ];
        let priority = InputPriority {
            priorities: vec!["builtin".into()],
            known: Vec::new(),
        };
        assert_eq!(
            resolve_input(&connected, params(Some("webcam"), true, &priority),),
            Some("webcam".into()),
        );
    }

    #[test]
    fn clamshell_ignored_when_not_active() {
        let connected = vec![
            device("builtin", "Built-in", TransportType::BuiltIn, true),
            device("webcam", "Webcam", TransportType::Usb, false),
        ];
        assert_eq!(
            resolve_input(&connected, params(Some("webcam"), false, &empty_priority()),),
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
            known: Vec::new(),
        };
        assert_eq!(
            resolve_input(&connected, params(None, false, &priority)),
            Some("hdmi".into()),
        );
    }

    #[test]
    fn saved_settings_with_a_hidden_list_still_load_and_follow_the_order() {
        let priority: InputPriority =
            serde_json::from_str(r#"{"priorities":["usb","builtin"],"hidden":["usb"],"known":[]}"#)
                .expect("settings saved by earlier versions");
        let connected = vec![
            device("builtin", "Built-in", TransportType::BuiltIn, true),
            device("usb", "USB", TransportType::Usb, false),
        ];
        assert_eq!(
            resolve_input(&connected, params(None, false, &priority)),
            Some("usb".into()),
        );
    }

    #[test]
    fn a_bluetooth_microphone_ranked_first_wins() {
        let connected = vec![
            device("bt", "AirPods", TransportType::Bluetooth, false),
            device("usb", "USB", TransportType::Usb, true),
        ];
        let priority = InputPriority {
            priorities: vec!["bt".into(), "usb".into()],
            known: Vec::new(),
        };
        assert_eq!(
            resolve_input(&connected, params(None, false, &priority)),
            Some("bt".into()),
        );
    }

    #[test]
    fn a_bluetooth_system_default_wins_without_a_ranking() {
        let connected = vec![
            device("builtin", "Built-in", TransportType::BuiltIn, false),
            device("bt", "AirPods", TransportType::Bluetooth, true),
        ];
        assert_eq!(
            resolve_input(&connected, params(None, false, &empty_priority())),
            Some("bt".into()),
        );
    }

    #[test]
    fn empty_connected_returns_none() {
        assert_eq!(
            resolve_input(&[], params(None, false, &empty_priority())),
            None,
        );
    }

    #[test]
    fn promote_moves_a_ranked_device_to_the_top_without_duplicating_it() {
        let mut priority = InputPriority {
            priorities: vec!["a".into(), "b".into(), "c".into()],
            known: Vec::new(),
        };
        priority.promote("c");
        assert_eq!(priority.priorities, ["c", "a", "b"]);
        priority.promote("new");
        assert_eq!(priority.priorities, ["new", "c", "a", "b"]);
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
