//! Port of `microphone-list.ts`'s pure list-building/reordering functions.
//! The actual priority *resolution* used at capture time stays in
//! `souffle_lib::audio::resolve_input` (not duplicated here) - this only
//! builds/edits the Settings picker's display list, exactly like the
//! Svelte version does client-side.

use souffle_lib::audio::{AudioInputDevice, InputPriority, TransportType};

#[derive(Debug, Clone, PartialEq)]
pub struct MicrophoneListEntry {
    pub uid: String,
    pub name: String,
    pub transport: TransportType,
    pub is_default: bool,
    pub connected: bool,
    pub hidden: bool,
    pub last_seen: Option<i64>,
}

/// Merge connected devices, remembered devices, and priority order.
pub fn build_microphone_list(
    connected: &[AudioInputDevice],
    priority: &InputPriority,
) -> Vec<MicrophoneListEntry> {
    let mut seen = std::collections::HashSet::new();
    let mut ordered = Vec::new();

    let mut push = |uid: &str| {
        if !seen.insert(uid.to_string()) {
            return;
        }
        let live = connected.iter().find(|d| d.uid == uid);
        let known = priority.known.iter().find(|k| k.uid == uid);
        ordered.push(MicrophoneListEntry {
            uid: uid.to_string(),
            name: live
                .map(|d| d.name.clone())
                .or_else(|| known.map(|k| k.name.clone()))
                .unwrap_or_else(|| uid.to_string()),
            transport: live.map(|d| d.transport).unwrap_or(TransportType::Unknown),
            is_default: live.map(|d| d.is_default).unwrap_or(false),
            connected: live.is_some(),
            hidden: priority.hidden.iter().any(|h| h == uid),
            last_seen: if live.is_some() {
                None
            } else {
                known.map(|k| k.last_seen)
            },
        });
    };

    for uid in &priority.priorities {
        push(uid);
    }
    for entry in &priority.known {
        push(&entry.uid);
    }
    for device in connected {
        push(&device.uid);
    }

    ordered
}

/// Drop one device from priorities, known, and hidden. No-op if absent from all three.
pub fn remove_known_device(priority: &InputPriority, uid: &str) -> InputPriority {
    InputPriority {
        priorities: priority
            .priorities
            .iter()
            .filter(|e| e.as_str() != uid)
            .cloned()
            .collect(),
        hidden: priority
            .hidden
            .iter()
            .filter(|e| e.as_str() != uid)
            .cloned()
            .collect(),
        known: priority
            .known
            .iter()
            .filter(|e| e.uid != uid)
            .cloned()
            .collect(),
    }
}

/// Swaps `uid` with its neighbor in `direction` (-1 up, +1 down). `None`
/// when `uid` is missing or already at that end of the list.
pub fn reorder_microphone_list(
    list: &[MicrophoneListEntry],
    uid: &str,
    direction: i32,
) -> Option<Vec<String>> {
    let index = list.iter().position(|e| e.uid == uid)? as i32;
    let target = index + direction;
    if target < 0 || target >= list.len() as i32 {
        return None;
    }
    let mut uids: Vec<String> = list.iter().map(|e| e.uid.clone()).collect();
    uids.swap(index as usize, target as usize);
    Some(uids)
}

pub fn transport_label(transport: TransportType) -> &'static str {
    match transport {
        TransportType::BuiltIn => "__BUILTIN__",
        TransportType::Usb => "__USB__",
        TransportType::Bluetooth | TransportType::BluetoothLe => "__BLUETOOTH__",
        TransportType::Virtual => "__VIRTUAL__",
        TransportType::Aggregate => "__AGGREGATE__",
        TransportType::Unknown => "__UNKNOWN__",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use souffle_lib::audio::KnownDevice;

    fn device(uid: &str, name: &str) -> AudioInputDevice {
        AudioInputDevice {
            uid: uid.to_string(),
            name: name.to_string(),
            transport: TransportType::Usb,
            is_default: false,
        }
    }

    #[test]
    fn orders_by_priority_then_known_then_connected() {
        let priority = InputPriority {
            priorities: vec!["b".to_string()],
            hidden: vec![],
            known: vec![KnownDevice {
                uid: "c".to_string(),
                name: "C".to_string(),
                last_seen: 1,
            }],
        };
        let connected = vec![device("a", "A"), device("b", "B")];
        let list = build_microphone_list(&connected, &priority);
        let uids: Vec<&str> = list.iter().map(|e| e.uid.as_str()).collect();
        assert_eq!(uids, vec!["b", "c", "a"]);
        assert!(list[0].connected);
        assert!(!list[1].connected);
    }

    #[test]
    fn reorder_swaps_with_neighbor() {
        let list = vec![
            MicrophoneListEntry {
                uid: "a".into(),
                name: "A".into(),
                transport: TransportType::Usb,
                is_default: false,
                connected: true,
                hidden: false,
                last_seen: None,
            },
            MicrophoneListEntry {
                uid: "b".into(),
                name: "B".into(),
                transport: TransportType::Usb,
                is_default: false,
                connected: true,
                hidden: false,
                last_seen: None,
            },
        ];
        assert_eq!(
            reorder_microphone_list(&list, "a", 1),
            Some(vec!["b".to_string(), "a".to_string()])
        );
        assert_eq!(reorder_microphone_list(&list, "a", -1), None);
    }

    #[test]
    fn remove_known_device_drops_from_all_three_lists() {
        let priority = InputPriority {
            priorities: vec!["a".to_string(), "b".to_string()],
            hidden: vec!["a".to_string()],
            known: vec![KnownDevice {
                uid: "a".to_string(),
                name: "A".to_string(),
                last_seen: 1,
            }],
        };
        let next = remove_known_device(&priority, "a");
        assert_eq!(next.priorities, vec!["b".to_string()]);
        assert!(next.hidden.is_empty());
        assert!(next.known.is_empty());
    }
}
