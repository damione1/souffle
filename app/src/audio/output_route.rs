//! Default-output-device introspection for system-audio capture.
//!
//! The system tap's aggregate device must reference the current default
//! output device, and echo cancellation is only useful when that output can
//! actually leak into the microphone: built-in speakers, not muted, and at
//! an audible volume. The tap delivers audio pre-volume, so a muted or
//! silenced speaker output still hands the canceller a full-level signal it
//! will never hear echoed back, which makes an unconverged canceller
//! suppress the mic instead.

#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2_core_audio::{
    AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress,
    kAudioDevicePropertyDataSource, kAudioDevicePropertyDeviceUID, kAudioDevicePropertyMute,
    kAudioDevicePropertyTransportType, kAudioDevicePropertyVolumeScalar,
    kAudioDeviceTransportTypeAggregate, kAudioDeviceTransportTypeAirPlay,
    kAudioDeviceTransportTypeBluetooth, kAudioDeviceTransportTypeBluetoothLE,
    kAudioDeviceTransportTypeBuiltIn, kAudioDeviceTransportTypeVirtual,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeOutput,
    kAudioObjectSystemObject,
};
use objc2_core_foundation::{CFRetained, CFString};

/// Built-in output data source: internal speaker ('ispk').
const DATA_SOURCE_INTERNAL_SPEAKER: u32 = u32::from_be_bytes(*b"ispk");
/// Output data source: headphones ('hdpn').
const DATA_SOURCE_HEADPHONES: u32 = u32::from_be_bytes(*b"hdpn");

fn global_address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn get_property<T>(
    object: AudioObjectID,
    mut address: AudioObjectPropertyAddress,
    out: &mut T,
) -> Result<(), String> {
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
    if status != 0 {
        return Err(format!(
            "AudioObjectGetPropertyData({:#x}) failed: {status}",
            address.mSelector
        ));
    }
    Ok(())
}

/// The current default output device.
pub fn default_output_device() -> Result<AudioObjectID, String> {
    let mut device: AudioObjectID = 0;
    get_property(
        kAudioObjectSystemObject as AudioObjectID,
        global_address(kAudioHardwarePropertyDefaultOutputDevice),
        &mut device,
    )?;
    if device == 0 {
        return Err("No default output device".into());
    }
    Ok(device)
}

/// The persistent UID of a device, as used in aggregate-device compositions.
pub fn device_uid(device: AudioObjectID) -> Result<String, String> {
    // The property hands back a +1 retained CFString.
    let mut uid: *const CFString = std::ptr::null();
    get_property(
        device,
        global_address(kAudioDevicePropertyDeviceUID),
        &mut uid,
    )?;
    let ptr = NonNull::new(uid.cast_mut()).ok_or("Device UID is null")?;
    let uid = unsafe { CFRetained::from_raw(ptr) };
    Ok(uid.to_string())
}

fn device_name(device: AudioObjectID) -> Option<String> {
    let mut name: *const CFString = std::ptr::null();
    get_property(device, global_address(kAudioObjectPropertyName), &mut name).ok()?;
    let ptr = NonNull::new(name.cast_mut())?;
    let name = unsafe { CFRetained::from_raw(ptr) };
    Some(name.to_string())
}

fn output_address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeOutput,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn name_looks_like_headphones(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("headphone")
        || lower.contains("headset")
        || lower.contains("airpod")
        || lower.contains("earbud")
        || lower.contains("earphone")
        || lower.contains("écouteur")
        || lower.contains("casque")
}

/// Pure decision on whether an output route with the given transport, optional
/// data-source FourCC, and optional device name can acoustically bleed into
/// the microphone.
///
/// - Built-in transport covers both internal speakers (`'ispk'`) and the 3.5mm
///   headphone jack (`'hdpn'`); only `'ispk'` (or no reported source when the
///   device name is not headphones) is treated as speakers.
/// - Bluetooth, Virtual, Aggregate, and AirPlay transports are excluded.
/// - External wired/digital transports (DisplayPort, HDMI, Thunderbolt, USB,
///   etc.) are treated as speaker-capable unless their data source is `'hdpn'`
///   or their device name indicates headphones/headset.
fn route_is_speakers(transport: u32, data_source: Option<u32>, name: Option<&str>) -> bool {
    if transport == 0
        || transport == kAudioDeviceTransportTypeBluetooth
        || transport == kAudioDeviceTransportTypeBluetoothLE
        || transport == kAudioDeviceTransportTypeVirtual
        || transport == kAudioDeviceTransportTypeAggregate
        || transport == kAudioDeviceTransportTypeAirPlay
    {
        return false;
    }

    if data_source == Some(DATA_SOURCE_HEADPHONES) {
        return false;
    }

    if name.is_some_and(name_looks_like_headphones) {
        return false;
    }

    if transport == kAudioDeviceTransportTypeBuiltIn {
        return match data_source {
            Some(source) => source == DATA_SOURCE_INTERNAL_SPEAKER,
            None => true,
        };
    }

    true
}

/// Whether the default output device routes to speakers (built-in or external
/// monitor/USB/dock speakers) rather than headphones or a non-speaker transport.
fn output_is_speakers(device: AudioObjectID) -> bool {
    let mut transport: u32 = 0;
    if get_property(
        device,
        global_address(kAudioDevicePropertyTransportType),
        &mut transport,
    )
    .is_err()
    {
        return false;
    }

    let mut source: u32 = 0;
    let data_source = get_property(
        device,
        output_address(kAudioDevicePropertyDataSource),
        &mut source,
    )
    .ok()
    .map(|()| source);

    let name = device_name(device);
    route_is_speakers(transport, data_source, name.as_deref())
}

/// Whether the default output device is muted. Treated as not muted if the
/// property can't be read (not every device implements it).
fn output_is_muted(device: AudioObjectID) -> bool {
    let mut muted: u32 = 0;
    match get_property(device, output_address(kAudioDevicePropertyMute), &mut muted) {
        Ok(()) => muted != 0,
        Err(_) => false,
    }
}

/// The default output device's scalar volume (0.0 to 1.0). Falls back from
/// the main element to channel 1, then assumes audible if neither can be
/// read (some devices only expose per-channel volume, others none at all).
fn output_volume(device: AudioObjectID) -> f32 {
    let mut volume: f32 = 0.0;
    if get_property(
        device,
        output_address(kAudioDevicePropertyVolumeScalar),
        &mut volume,
    )
    .is_ok()
    {
        return volume;
    }

    let channel_one = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyVolumeScalar,
        mScope: kAudioObjectPropertyScopeOutput,
        mElement: 1,
    };
    match get_property(device, channel_one, &mut volume) {
        Ok(()) => volume,
        Err(_) => 1.0,
    }
}

/// Minimum scalar volume treated as audible. Below this, the render signal
/// reaching the speakers is effectively silence.
const AUDIBLE_VOLUME_THRESHOLD: f32 = 0.01;

/// Pure decision: can output audio acoustically leak back into the mic?
/// Only true for unmuted, audible speaker routes; headphones and non-acoustic
/// transports can't leak regardless of volume or mute state.
fn can_leak(is_speakers: bool, muted: bool, volume: f32) -> bool {
    is_speakers && !muted && volume > AUDIBLE_VOLUME_THRESHOLD
}

/// Whether the default output can acoustically leak into the microphone
/// right now: speaker route, unmuted, and at an audible volume. This is
/// the only case where echo cancellation does useful work; running it
/// against a muted or silenced output starves the canceller of any real
/// echo to converge on and it ends up suppressing the mic instead.
pub fn output_can_leak_into_mic() -> bool {
    let Ok(device) = default_output_device() else {
        return false;
    };

    let is_speakers = output_is_speakers(device);
    if !is_speakers {
        return false;
    }

    can_leak(is_speakers, output_is_muted(device), output_volume(device))
}

#[cfg(test)]
mod tests {
    use objc2_core_audio::{
        kAudioDeviceTransportTypeAggregate, kAudioDeviceTransportTypeAirPlay,
        kAudioDeviceTransportTypeBluetooth, kAudioDeviceTransportTypeBluetoothLE,
        kAudioDeviceTransportTypeBuiltIn, kAudioDeviceTransportTypeDisplayPort,
        kAudioDeviceTransportTypeHDMI, kAudioDeviceTransportTypeThunderbolt,
        kAudioDeviceTransportTypeUSB, kAudioDeviceTransportTypeVirtual,
    };

    use super::{
        DATA_SOURCE_HEADPHONES, DATA_SOURCE_INTERNAL_SPEAKER, can_leak, route_is_speakers,
    };

    #[test]
    fn speakers_unmuted_audible_can_leak() {
        assert!(can_leak(true, false, 1.0));
    }

    #[test]
    fn muted_speakers_cannot_leak() {
        assert!(!can_leak(true, true, 1.0));
    }

    #[test]
    fn zero_volume_speakers_cannot_leak() {
        assert!(!can_leak(true, false, 0.0));
    }

    #[test]
    fn headphones_cannot_leak() {
        assert!(!can_leak(false, false, 1.0));
    }

    #[test]
    fn builtin_internal_speaker_is_speakers() {
        assert!(route_is_speakers(
            kAudioDeviceTransportTypeBuiltIn,
            Some(DATA_SOURCE_INTERNAL_SPEAKER),
            Some("MacBook Pro Speakers"),
        ));
        assert!(route_is_speakers(
            kAudioDeviceTransportTypeBuiltIn,
            None,
            Some("MacBook Pro Speakers"),
        ));
    }

    #[test]
    fn builtin_headphone_jack_is_not_speakers() {
        assert!(!route_is_speakers(
            kAudioDeviceTransportTypeBuiltIn,
            Some(DATA_SOURCE_HEADPHONES),
            Some("External Headphones"),
        ));
    }

    #[test]
    fn external_monitor_and_usb_speakers_are_speakers() {
        assert!(route_is_speakers(
            kAudioDeviceTransportTypeDisplayPort,
            None,
            Some("Studio Display Speakers"),
        ));
        assert!(route_is_speakers(
            kAudioDeviceTransportTypeHDMI,
            None,
            Some("LG UltraFine"),
        ));
        assert!(route_is_speakers(
            kAudioDeviceTransportTypeThunderbolt,
            None,
            Some("Apple Thunderbolt Display"),
        ));
        assert!(route_is_speakers(
            kAudioDeviceTransportTypeUSB,
            None,
            Some("Jabra Speak 710"),
        ));
    }

    #[test]
    fn usb_headphones_and_non_speaker_transports_are_excluded() {
        assert!(!route_is_speakers(
            kAudioDeviceTransportTypeUSB,
            Some(DATA_SOURCE_HEADPHONES),
            Some("USB Audio"),
        ));
        assert!(!route_is_speakers(
            kAudioDeviceTransportTypeUSB,
            None,
            Some("Logitech USB Headset"),
        ));
        assert!(!route_is_speakers(
            kAudioDeviceTransportTypeBluetooth,
            None,
            Some("AirPods Pro"),
        ));
        assert!(!route_is_speakers(
            kAudioDeviceTransportTypeBluetoothLE,
            None,
            Some("LE Audio"),
        ));
        assert!(!route_is_speakers(
            kAudioDeviceTransportTypeVirtual,
            None,
            Some("BlackHole 2ch"),
        ));
        assert!(!route_is_speakers(
            kAudioDeviceTransportTypeAggregate,
            None,
            Some("Multi-Output Device"),
        ));
        assert!(!route_is_speakers(
            kAudioDeviceTransportTypeAirPlay,
            None,
            Some("Living Room HomePod"),
        ));
    }
}
