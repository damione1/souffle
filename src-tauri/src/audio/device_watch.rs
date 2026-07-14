//! CoreAudio input-device observability.
//!
//! Diagnostic groundwork for two problems: understanding why a Bluetooth
//! headset gets forced into HFP mono while the app is running, and a future
//! input-priority system (Bluetooth headset > built-in mic > USB mic,
//! depending on lid state). Nothing here changes audio routing; it only
//! logs what CoreAudio reports so the live log in Settings > Diagnostics can
//! be watched while reproducing the issue.
//!
//! Property listeners are registered once on the system object and left
//! running for the app's lifetime — see [`start`]. The listener blocks fire
//! on a CoreAudio-owned dispatch queue and must stay minimal and
//! panic-free, so each callback only forwards a lightweight event over a
//! channel; the actual work (querying device properties, diffing against
//! the previous snapshot, the `ioreg` clamshell probe, and logging) happens
//! on a dedicated worker thread.

#![cfg(target_os = "macos")]

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::mpsc::{Receiver, Sender, channel};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2_core_audio::{
    AudioObjectAddPropertyListenerBlock, AudioObjectGetPropertyData,
    AudioObjectGetPropertyDataSize, AudioObjectID, AudioObjectPropertyAddress,
    kAudioDevicePropertyDeviceUID, kAudioDevicePropertyStreams,
    kAudioDevicePropertyTransportType, kAudioDeviceTransportTypeAggregate,
    kAudioDeviceTransportTypeBluetooth, kAudioDeviceTransportTypeBluetoothLE,
    kAudioDeviceTransportTypeBuiltIn, kAudioDeviceTransportTypeUSB,
    kAudioDeviceTransportTypeVirtual, kAudioHardwarePropertyDefaultInputDevice,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyDevices,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeInput, kAudioObjectSystemObject,
};
use objc2_core_foundation::{CFRetained, CFString};
use tracing::{info, warn};

use crate::audio::device::{AudioInputDevice, TransportType, is_souffle_tap_device};
use crate::power::is_clamshell;

type ListenerBlock = RcBlock<dyn Fn(u32, NonNull<AudioObjectPropertyAddress>)>;

/// Identity of a CoreAudio device as logged: name and human transport.
#[derive(Clone, Debug, PartialEq, Eq)]
struct DeviceInfo {
    name: String,
    transport: String,
}

/// Which property changed, forwarded from a listener block to the worker
/// thread. Carries no data itself — the worker re-queries CoreAudio, which
/// is safe from any thread and keeps the callback itself trivial.
#[derive(Clone, Copy)]
enum ChangeKind {
    DeviceList,
    DefaultInput,
    DefaultOutput,
}

/// Keeps the property listeners and their dispatch queue alive. Dropping
/// this would let CoreAudio tear the listeners down, so [`start`]'s caller
/// is expected to leak it for the app's lifetime (mirrors the observer
/// token handling in `power::install_sleep_observers`).
pub struct DeviceWatchHandle {
    _queue: DispatchRetained<DispatchQueue>,
    _blocks: Vec<ListenerBlock>,
}

/// Start watching CoreAudio input-device changes. Logs an initial snapshot
/// immediately, then keeps logging device arrivals/removals and default
/// input/output changes for as long as the returned handle is kept alive.
/// Call once, from app setup.
pub fn start() -> DeviceWatchHandle {
    let (tx, rx) = channel::<ChangeKind>();
    let queue = DispatchQueue::new("com.souffle.device-watch", None);

    let blocks = vec![
        add_listener(
            kAudioHardwarePropertyDevices,
            &queue,
            tx.clone(),
            ChangeKind::DeviceList,
        ),
        add_listener(
            kAudioHardwarePropertyDefaultInputDevice,
            &queue,
            tx.clone(),
            ChangeKind::DefaultInput,
        ),
        add_listener(
            kAudioHardwarePropertyDefaultOutputDevice,
            &queue,
            tx,
            ChangeKind::DefaultOutput,
        ),
    ];

    std::thread::Builder::new()
        .name("device-watch".into())
        .spawn(move || run_worker(rx))
        .expect("failed to spawn device-watch worker thread");

    DeviceWatchHandle {
        _queue: queue,
        _blocks: blocks,
    }
}

/// Register one property listener on the system object. The block itself
/// does no CoreAudio work: it just forwards `kind` over the channel, so it
/// can't wedge or panic on the CoreAudio dispatch queue it runs on.
fn add_listener(
    selector: u32,
    queue: &DispatchQueue,
    tx: Sender<ChangeKind>,
    kind: ChangeKind,
) -> ListenerBlock {
    let block: ListenerBlock = RcBlock::new(
        move |_count: u32, _addresses: NonNull<AudioObjectPropertyAddress>| {
            let _ = tx.send(kind);
        },
    );
    let mut address = global_address(selector);
    let status = unsafe {
        AudioObjectAddPropertyListenerBlock(
            kAudioObjectSystemObject as AudioObjectID,
            NonNull::from(&mut address),
            Some(queue),
            RcBlock::as_ptr(&block),
        )
    };
    if status != 0 {
        warn!(selector = format!("{selector:#x}"), status, "Failed to register CoreAudio device listener");
    }
    block
}

/// Owns the previous snapshots and reacts to change events. Runs on its own
/// thread for the app's lifetime; all the actual CoreAudio querying and
/// `ioreg` clamshell probing happens here, off the CoreAudio dispatch queue.
fn run_worker(rx: Receiver<ChangeKind>) {
    let mut devices = enumerate_input_devices();
    let mut default_input = default_device_info(kAudioHardwarePropertyDefaultInputDevice);
    let mut default_output = default_device_info(kAudioHardwarePropertyDefaultOutputDevice);
    log_initial_snapshot(&devices, &default_input, &default_output);

    for kind in rx {
        match kind {
            ChangeKind::DeviceList => {
                let current = enumerate_input_devices();
                log_device_diff(&devices, &current);
                devices = current;
            }
            ChangeKind::DefaultInput => {
                let current = default_device_info(kAudioHardwarePropertyDefaultInputDevice);
                log_default_input_change(&default_input, &current);
                default_input = current;
            }
            ChangeKind::DefaultOutput => {
                let current = default_device_info(kAudioHardwarePropertyDefaultOutputDevice);
                log_default_output_change(&default_output, &current);
                default_output = current;
            }
        }
    }
}

fn log_initial_snapshot(
    devices: &HashMap<AudioObjectID, DeviceInfo>,
    default_input: &Option<DeviceInfo>,
    default_output: &Option<DeviceInfo>,
) {
    let lid_closed = is_clamshell();
    for info in devices.values() {
        info!(device = %info.name, transport = %info.transport, "Audio input device present at startup");
    }
    let (input_device, input_transport) = describe(default_input);
    info!(device = input_device, transport = input_transport, lid_closed, "Default input device at startup");
    let (output_device, output_transport) = describe(default_output);
    info!(device = output_device, transport = output_transport, "Default output device at startup");
    info!(lid_closed, "Lid state at startup");
}

fn log_device_diff(old: &HashMap<AudioObjectID, DeviceInfo>, new: &HashMap<AudioObjectID, DeviceInfo>) {
    let diff = diff_devices(old, new);
    if diff.arrived.is_empty() && diff.removed.is_empty() {
        return;
    }
    let lid_closed = is_clamshell();
    for (_, info) in &diff.arrived {
        info!(device = %info.name, transport = %info.transport, lid_closed, "Audio input device connected");
    }
    for (_, info) in &diff.removed {
        info!(device = %info.name, transport = %info.transport, lid_closed, "Audio input device disconnected");
    }
}

fn log_default_input_change(old: &Option<DeviceInfo>, new: &Option<DeviceInfo>) {
    if old == new {
        return;
    }
    let (old_device, old_transport) = describe(old);
    let (new_device, new_transport) = describe(new);
    let lid_closed = is_clamshell();
    info!(
        old_device,
        old_transport,
        new_device,
        new_transport,
        lid_closed,
        "Default input device changed"
    );
}

fn log_default_output_change(old: &Option<DeviceInfo>, new: &Option<DeviceInfo>) {
    if old == new {
        return;
    }
    let (old_device, old_transport) = describe(old);
    let (new_device, new_transport) = describe(new);
    info!(
        old_device,
        old_transport,
        new_device,
        new_transport,
        "Default output device changed"
    );
}

/// `(name, transport)` for logging, or `("none", "none")` when there's no
/// default device.
fn describe(info: &Option<DeviceInfo>) -> (&str, &str) {
    match info {
        Some(d) => (d.name.as_str(), d.transport.as_str()),
        None => ("none", "none"),
    }
}

struct DeviceDiff {
    arrived: Vec<(AudioObjectID, DeviceInfo)>,
    removed: Vec<(AudioObjectID, DeviceInfo)>,
}

/// Pure set diff between two input-device snapshots. Removed devices report
/// the name/transport captured before they disappeared — the device object
/// itself may no longer be queryable by the time this runs.
fn diff_devices(
    old: &HashMap<AudioObjectID, DeviceInfo>,
    new: &HashMap<AudioObjectID, DeviceInfo>,
) -> DeviceDiff {
    let arrived = new
        .iter()
        .filter(|(id, _)| !old.contains_key(id))
        .map(|(&id, info)| (id, info.clone()))
        .collect();
    let removed = old
        .iter()
        .filter(|(id, _)| !new.contains_key(id))
        .map(|(&id, info)| (id, info.clone()))
        .collect();
    DeviceDiff { arrived, removed }
}

fn map_transport(transport: u32) -> TransportType {
    match transport {
        t if t == kAudioDeviceTransportTypeBuiltIn => TransportType::BuiltIn,
        t if t == kAudioDeviceTransportTypeBluetooth => TransportType::Bluetooth,
        t if t == kAudioDeviceTransportTypeBluetoothLE => TransportType::BluetoothLe,
        t if t == kAudioDeviceTransportTypeUSB => TransportType::Usb,
        t if t == kAudioDeviceTransportTypeVirtual => TransportType::Virtual,
        t if t == kAudioDeviceTransportTypeAggregate => TransportType::Aggregate,
        _ => TransportType::Unknown,
    }
}

/// Human transport label for logging. Falls back to the raw four-char-code
/// hex value for transports not worth naming individually.
fn transport_name(transport: TransportType) -> String {
    match transport {
        TransportType::BuiltIn => "built-in".into(),
        TransportType::Bluetooth => "bluetooth".into(),
        TransportType::BluetoothLe => "bluetooth-le".into(),
        TransportType::Usb => "usb".into(),
        TransportType::Virtual => "virtual".into(),
        TransportType::Aggregate => "aggregate".into(),
        TransportType::Unknown => "unknown".into(),
    }
}

fn global_address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn input_scope_address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeInput,
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

/// Size in bytes of a property's data, or 0 if it can't be read (including
/// "device doesn't have this property", e.g. streams in a scope it doesn't
/// support).
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

/// The device ID list for a variable-length `AudioObjectID` array property
/// (e.g. `kAudioHardwarePropertyDevices`).
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

fn device_name(device: AudioObjectID) -> String {
    let mut name_ptr: *const CFString = std::ptr::null();
    if !get_property(device, global_address(kAudioObjectPropertyName), &mut name_ptr) {
        return format!("device#{device}");
    }
    match NonNull::new(name_ptr.cast_mut()) {
        // The property hands back a +1 retained CFString.
        Some(ptr) => unsafe { CFRetained::from_raw(ptr) }.to_string(),
        None => format!("device#{device}"),
    }
}

fn device_uid(device: AudioObjectID) -> String {
    let mut uid_ptr: *const CFString = std::ptr::null();
    if !get_property(device, global_address(kAudioDevicePropertyDeviceUID), &mut uid_ptr) {
        return format!("device#{device}");
    }
    match NonNull::new(uid_ptr.cast_mut()) {
        Some(ptr) => unsafe { CFRetained::from_raw(ptr) }.to_string(),
        None => format!("device#{device}"),
    }
}

fn device_transport(device: AudioObjectID) -> TransportType {
    let mut transport: u32 = 0;
    get_property(
        device,
        global_address(kAudioDevicePropertyTransportType),
        &mut transport,
    );
    map_transport(transport)
}

/// Input-capable = has at least one stream in the input scope.
fn is_input_capable(device: AudioObjectID) -> bool {
    property_data_size(device, input_scope_address(kAudioDevicePropertyStreams)) > 0
}

fn device_info(device: AudioObjectID) -> DeviceInfo {
    DeviceInfo {
        name: device_name(device),
        transport: transport_name(device_transport(device)),
    }
}

fn input_device(device: AudioObjectID, is_default: bool) -> AudioInputDevice {
    AudioInputDevice {
        uid: device_uid(device),
        name: device_name(device),
        transport: device_transport(device),
        is_default,
    }
}

fn enumerate_input_devices() -> HashMap<AudioObjectID, DeviceInfo> {
    device_ids(
        kAudioObjectSystemObject as AudioObjectID,
        global_address(kAudioHardwarePropertyDevices),
    )
    .into_iter()
    .filter(|&id| is_input_capable(id))
    .map(|id| (id, device_info(id)))
    .collect()
}

fn default_device_id(selector: u32) -> Option<AudioObjectID> {
    let mut device: AudioObjectID = 0;
    if !get_property(kAudioObjectSystemObject as AudioObjectID, global_address(selector), &mut device) {
        return None;
    }
    (device != 0).then_some(device)
}

fn default_device_info(selector: u32) -> Option<DeviceInfo> {
    default_device_id(selector).map(device_info)
}

/// The system's current default input device id, or `None` if there isn't one.
pub(crate) fn default_input_device_id() -> Option<AudioObjectID> {
    default_device_id(kAudioHardwarePropertyDefaultInputDevice)
}

/// Every input-capable device with stable UID and transport. Pure CoreAudio
/// property queries only (device list, stream count, name, uid, transport) —
/// unlike cpal's `Host::input_devices()`, this never queries a device's
/// supported stream formats, so it can't open an AudioUnit on a device and
/// force a Bluetooth headset out of A2DP. Used for the Settings device list.
pub(crate) fn list_devices() -> Vec<AudioInputDevice> {
    let default = default_input_device_id();
    device_ids(
        kAudioObjectSystemObject as AudioObjectID,
        global_address(kAudioHardwarePropertyDevices),
    )
    .into_iter()
    .filter(|&id| is_input_capable(id))
    .map(|id| input_device(id, Some(id) == default))
    .filter(|device| !is_souffle_tap_device(&device.name))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, transport: &str) -> DeviceInfo {
        DeviceInfo {
            name: name.to_string(),
            transport: transport.to_string(),
        }
    }

    #[test]
    fn diff_detects_arrival() {
        let old = HashMap::new();
        let mut new = HashMap::new();
        new.insert(42, info("Bose 700", "bluetooth"));

        let diff = diff_devices(&old, &new);

        assert_eq!(diff.arrived.len(), 1);
        assert_eq!(diff.arrived[0].1.name, "Bose 700");
        assert!(diff.removed.is_empty());
    }

    #[test]
    fn diff_detects_removal() {
        let mut old = HashMap::new();
        old.insert(42, info("Bose 700", "bluetooth"));
        let new = HashMap::new();

        let diff = diff_devices(&old, &new);

        assert!(diff.arrived.is_empty());
        assert_eq!(diff.removed.len(), 1);
        assert_eq!(diff.removed[0].1.name, "Bose 700");
    }

    #[test]
    fn diff_ignores_unchanged_devices() {
        let mut old = HashMap::new();
        old.insert(1, info("MacBook Pro Microphone", "built-in"));
        let new = old.clone();

        let diff = diff_devices(&old, &new);

        assert!(diff.arrived.is_empty());
        assert!(diff.removed.is_empty());
    }

    #[test]
    fn diff_detects_swap() {
        let mut old = HashMap::new();
        old.insert(1, info("Built-in Microphone", "built-in"));
        let mut new = HashMap::new();
        new.insert(2, info("Bose 700", "bluetooth"));

        let diff = diff_devices(&old, &new);

        assert_eq!(diff.arrived.len(), 1);
        assert_eq!(diff.removed.len(), 1);
    }

    #[test]
    fn transport_maps_known_types() {
        assert_eq!(transport_name(TransportType::BuiltIn), "built-in");
        assert_eq!(transport_name(TransportType::Bluetooth), "bluetooth");
        assert_eq!(transport_name(TransportType::BluetoothLe), "bluetooth-le");
        assert_eq!(transport_name(TransportType::Usb), "usb");
        assert_eq!(transport_name(TransportType::Virtual), "virtual");
        assert_eq!(transport_name(TransportType::Aggregate), "aggregate");
        assert_eq!(transport_name(TransportType::Unknown), "unknown");
    }

    #[test]
    fn map_transport_covers_coreaudio_codes() {
        assert_eq!(map_transport(kAudioDeviceTransportTypeBuiltIn), TransportType::BuiltIn);
        assert_eq!(
            map_transport(kAudioDeviceTransportTypeBluetooth),
            TransportType::Bluetooth
        );
        assert_eq!(map_transport(0xdead_beef), TransportType::Unknown);
    }

    #[test]
    fn describe_none_is_placeholder() {
        assert_eq!(describe(&None), ("none", "none"));
    }

    #[test]
    fn describe_some_returns_fields() {
        let d = Some(info("USB Mic", "usb"));
        assert_eq!(describe(&d), ("USB Mic", "usb"));
    }
}
