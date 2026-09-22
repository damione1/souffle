//! Pure snapshot-diff for live microphone-route toasts.

use std::collections::HashSet;
use std::sync::Mutex;

use super::device::{AudioInputDevice, TransportType, is_souffle_tap_device};
use crate::app_events::{InputRouteNotice, InputRouteReason};

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceSnap {
    uid: String,
    name: String,
    transport: TransportType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RouteSnapshot {
    devices: Vec<DeviceSnap>,
    resolved_uid: Option<String>,
}

static PREVIOUS: Mutex<Option<RouteSnapshot>> = Mutex::new(None);

fn snapshot_of(devices: &[AudioInputDevice], resolved_uid: Option<String>) -> RouteSnapshot {
    RouteSnapshot {
        devices: devices
            .iter()
            .map(|device| DeviceSnap {
                uid: device.uid.clone(),
                name: device.name.clone(),
                transport: device.transport,
            })
            .collect(),
        resolved_uid,
    }
}

fn device_by_uid<'a>(snapshot: &'a RouteSnapshot, uid: &str) -> Option<&'a DeviceSnap> {
    snapshot.devices.iter().find(|device| device.uid == uid)
}

fn name_of(snapshot: &RouteSnapshot, uid: Option<&str>) -> Option<String> {
    uid.and_then(|uid| device_by_uid(snapshot, uid).map(|device| device.name.clone()))
}

fn transport_of(snapshot: &RouteSnapshot, uid: Option<&str>) -> Option<TransportType> {
    uid.and_then(|uid| device_by_uid(snapshot, uid).map(|device| device.transport))
}

fn uid_set(snapshot: &RouteSnapshot) -> HashSet<&str> {
    snapshot
        .devices
        .iter()
        .map(|device| device.uid.as_str())
        .collect()
}

fn skip_connected_notice(device: &DeviceSnap) -> bool {
    if is_souffle_tap_device(&device.name) {
        return true;
    }
    matches!(
        device.transport,
        TransportType::Virtual | TransportType::Aggregate
    ) && device.name.to_ascii_lowercase().contains("souffle")
}

fn notices_for_change(
    previous: Option<&RouteSnapshot>,
    next: &RouteSnapshot,
) -> Vec<InputRouteNotice> {
    let Some(previous) = previous else {
        return Vec::new();
    };

    if uid_set(previous) == uid_set(next) && previous.resolved_uid == next.resolved_uid {
        return Vec::new();
    }

    let mut notices = Vec::new();
    let switched_to = next.resolved_uid.as_deref();

    if previous.resolved_uid != next.resolved_uid {
        if let Some(to_uid) = next.resolved_uid.as_deref() {
            notices.push(InputRouteNotice {
                reason: InputRouteReason::Switched,
                from_name: name_of(previous, previous.resolved_uid.as_deref()),
                to_name: name_of(next, Some(to_uid)),
                to_uid: Some(to_uid.to_string()),
                transport: transport_of(next, Some(to_uid)),
            });
        } else {
            notices.push(InputRouteNotice {
                reason: InputRouteReason::Lost,
                from_name: name_of(previous, previous.resolved_uid.as_deref()),
                to_name: None,
                to_uid: None,
                transport: None,
            });
        }
    }

    let previous_uids = uid_set(previous);
    for device in &next.devices {
        if previous_uids.contains(device.uid.as_str()) {
            continue;
        }
        if switched_to == Some(device.uid.as_str()) {
            continue;
        }
        if skip_connected_notice(device) {
            continue;
        }
        notices.push(InputRouteNotice {
            reason: InputRouteReason::Connected,
            from_name: None,
            to_name: Some(device.name.clone()),
            to_uid: Some(device.uid.clone()),
            transport: Some(device.transport),
        });
    }

    notices
}

/// Diff `devices` + resolved capture UID against the last observation.
/// The first call stores the snapshot and emits nothing (skip boot).
pub fn observe_and_notices(
    devices: &[AudioInputDevice],
    resolved_uid: Option<String>,
) -> Vec<InputRouteNotice> {
    let next = snapshot_of(devices, resolved_uid);
    let mut previous = match PREVIOUS.lock() {
        Ok(guard) => guard,
        // Best-effort toasts: a poisoned snapshot must not skip the next plug.
        Err(poisoned) => poisoned.into_inner(),
    };
    let notices = notices_for_change(previous.as_ref(), &next);
    *previous = Some(next);
    notices
}

#[cfg(test)]
pub fn reset_for_test() {
    let mut previous = match PREVIOUS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *previous = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(uid: &str, name: &str, transport: TransportType) -> AudioInputDevice {
        AudioInputDevice {
            uid: uid.into(),
            name: name.into(),
            transport,
            is_default: uid == "builtin",
        }
    }

    fn builtin() -> AudioInputDevice {
        device("builtin", "MacBook Pro Microphone", TransportType::BuiltIn)
    }

    fn usb() -> AudioInputDevice {
        device("usb", "USB Webcam Mic", TransportType::Usb)
    }

    fn bluetooth() -> AudioInputDevice {
        device("bt", "AirPods Pro", TransportType::Bluetooth)
    }

    fn bluetooth_le() -> AudioInputDevice {
        device("btle", "AirPods Pro", TransportType::BluetoothLe)
    }

    fn snap(devices: &[AudioInputDevice], resolved: Option<&str>) -> RouteSnapshot {
        snapshot_of(devices, resolved.map(str::to_string))
    }

    fn reasons(notices: &[InputRouteNotice]) -> Vec<InputRouteReason> {
        notices.iter().map(|notice| notice.reason).collect()
    }

    #[test]
    fn first_observation_emits_nothing() {
        let next = snap(&[builtin()], Some("builtin"));
        assert!(notices_for_change(None, &next).is_empty());
    }

    #[test]
    fn switched_emits_from_to_and_not_connected_for_target() {
        let previous = snap(&[builtin()], Some("builtin"));
        let next = snap(&[builtin(), usb()], Some("usb"));
        let notices = notices_for_change(Some(&previous), &next);
        assert_eq!(reasons(&notices), vec![InputRouteReason::Switched]);
        assert_eq!(
            notices[0].from_name.as_deref(),
            Some("MacBook Pro Microphone")
        );
        assert_eq!(notices[0].to_name.as_deref(), Some("USB Webcam Mic"));
        assert_eq!(notices[0].to_uid.as_deref(), Some("usb"));
        assert_eq!(notices[0].transport, Some(TransportType::Usb));
    }

    #[test]
    fn lost_when_resolved_becomes_none() {
        let previous = snap(&[usb()], Some("usb"));
        let next = snap(&[], None);
        let notices = notices_for_change(Some(&previous), &next);
        assert_eq!(reasons(&notices), vec![InputRouteReason::Lost]);
        assert_eq!(notices[0].from_name.as_deref(), Some("USB Webcam Mic"));
        assert!(notices[0].to_name.is_none());
        assert!(notices[0].to_uid.is_none());
    }

    #[test]
    fn lost_keeps_from_name_when_other_devices_remain_but_none_resolve() {
        let previous = snap(&[builtin(), bluetooth()], Some("builtin"));
        let next = snap(&[bluetooth()], None);
        let notices = notices_for_change(Some(&previous), &next);
        assert_eq!(reasons(&notices), vec![InputRouteReason::Lost]);
        assert_eq!(
            notices[0].from_name.as_deref(),
            Some("MacBook Pro Microphone")
        );
    }

    #[test]
    fn connected_for_new_device_that_is_not_the_target() {
        let previous = snap(&[builtin()], Some("builtin"));
        let next = snap(&[builtin(), bluetooth()], Some("builtin"));
        let notices = notices_for_change(Some(&previous), &next);
        assert_eq!(reasons(&notices), vec![InputRouteReason::Connected]);
        assert_eq!(notices[0].to_name.as_deref(), Some("AirPods Pro"));
        assert_eq!(notices[0].to_uid.as_deref(), Some("bt"));
        assert_eq!(notices[0].transport, Some(TransportType::Bluetooth));
        assert!(notices[0].from_name.is_none());
    }

    #[test]
    fn connected_for_bluetooth_le_when_not_selected() {
        let previous = snap(&[builtin()], Some("builtin"));
        let next = snap(&[builtin(), bluetooth_le()], Some("builtin"));
        let notices = notices_for_change(Some(&previous), &next);
        assert_eq!(notices[0].transport, Some(TransportType::BluetoothLe));
        assert_eq!(notices[0].reason, InputRouteReason::Connected);
    }

    #[test]
    fn skip_souffle_tap_aggregates() {
        let tap = device("tap", "Souffle Tap", TransportType::Aggregate);
        let previous = snap(&[builtin()], Some("builtin"));
        let next = snap(&[builtin(), tap], Some("builtin"));
        assert!(notices_for_change(Some(&previous), &next).is_empty());
    }

    #[test]
    fn skip_numbered_souffle_tap_leftovers() {
        let tap = device("tap-2", "Souffle Tap 2", TransportType::Aggregate);
        let previous = snap(&[builtin()], Some("builtin"));
        let next = snap(&[builtin(), tap], Some("builtin"));
        assert!(notices_for_change(Some(&previous), &next).is_empty());
    }

    #[test]
    fn skip_virtual_devices_that_look_like_the_tap() {
        let virtual_tap = device("virt", "Souffle Mix", TransportType::Virtual);
        let previous = snap(&[builtin()], Some("builtin"));
        let next = snap(&[builtin(), virtual_tap], Some("builtin"));
        assert!(notices_for_change(Some(&previous), &next).is_empty());
    }

    #[test]
    fn fallback_switch_is_switched_not_lost() {
        let previous = snap(&[builtin(), usb()], Some("usb"));
        let next = snap(&[builtin()], Some("builtin"));
        let notices = notices_for_change(Some(&previous), &next);
        assert_eq!(reasons(&notices), vec![InputRouteReason::Switched]);
        assert_eq!(notices[0].from_name.as_deref(), Some("USB Webcam Mic"));
        assert_eq!(
            notices[0].to_name.as_deref(),
            Some("MacBook Pro Microphone")
        );
    }

    #[test]
    fn name_refresh_of_same_uids_is_ignored() {
        let previous = snap(&[builtin()], Some("builtin"));
        let renamed = device("builtin", "Built-in Microphone", TransportType::BuiltIn);
        let next = snap(&[renamed], Some("builtin"));
        assert!(notices_for_change(Some(&previous), &next).is_empty());
    }

    #[test]
    fn same_uid_set_and_resolved_is_noop() {
        let previous = snap(&[builtin(), usb()], Some("usb"));
        let next = snap(&[usb(), builtin()], Some("usb"));
        assert!(notices_for_change(Some(&previous), &next).is_empty());
    }

    #[test]
    fn switch_to_already_connected_device_is_switched_only() {
        let previous = snap(&[builtin(), usb()], Some("builtin"));
        let next = snap(&[builtin(), usb()], Some("usb"));
        let notices = notices_for_change(Some(&previous), &next);
        assert_eq!(reasons(&notices), vec![InputRouteReason::Switched]);
    }

    #[test]
    fn new_target_plus_other_arrival_emits_switched_then_connected() {
        let previous = snap(&[builtin()], Some("builtin"));
        let next = snap(&[builtin(), usb(), bluetooth()], Some("usb"));
        let notices = notices_for_change(Some(&previous), &next);
        assert_eq!(
            reasons(&notices),
            vec![InputRouteReason::Switched, InputRouteReason::Connected]
        );
        assert_eq!(notices[0].to_uid.as_deref(), Some("usb"));
        assert_eq!(notices[1].to_uid.as_deref(), Some("bt"));
    }

    #[test]
    fn observe_and_notices_skips_boot_then_emits() {
        reset_for_test();
        assert!(observe_and_notices(&[builtin()], Some("builtin".into())).is_empty());
        let notices = observe_and_notices(&[builtin(), usb()], Some("usb".into()));
        assert_eq!(reasons(&notices), vec![InputRouteReason::Switched]);
        reset_for_test();
    }
}
