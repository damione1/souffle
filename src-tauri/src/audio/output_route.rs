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
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
    AudioObjectPropertyAddress, AudioObjectSetPropertyData, kAudioDevicePropertyDataSource,
    kAudioDevicePropertyDataSources, kAudioDevicePropertyDeviceUID, kAudioDevicePropertyMute,
    kAudioDevicePropertyTransportType, kAudioDevicePropertyVolumeScalar,
    kAudioDeviceTransportTypeBluetooth, kAudioDeviceTransportTypeBluetoothLE,
    kAudioDeviceTransportTypeBuiltIn, kAudioHardwarePropertyDefaultOutputDevice,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject,
};
use objc2_core_foundation::{CFRetained, CFString};

/// Built-in output data source: internal speaker ('ispk').
const DATA_SOURCE_INTERNAL_SPEAKER: u32 = u32::from_be_bytes(*b"ispk");
/// Bluetooth stereo output ('a2dp') versus hands-free SCO ('hfp ').
const DATA_SOURCE_A2DP: u32 = u32::from_be_bytes(*b"a2dp");
const DATA_SOURCE_HFP: u32 = u32::from_be_bytes(*b"hfp ");

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

fn output_address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeOutput,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Whether the default output device routes to the built-in speakers rather
/// than headphones or an external device. Built-in transport covers both
/// the speakers and the headphone jack; the data source ('ispk' vs 'hdpn')
/// tells them apart. If the device doesn't report one, assume speakers.
fn output_is_builtin_speakers(device: AudioObjectID) -> bool {
    let mut transport: u32 = 0;
    if get_property(
        device,
        global_address(kAudioDevicePropertyTransportType),
        &mut transport,
    )
    .is_err()
        || transport != kAudioDeviceTransportTypeBuiltIn
    {
        return false;
    }

    let mut source: u32 = 0;
    match get_property(
        device,
        output_address(kAudioDevicePropertyDataSource),
        &mut source,
    ) {
        Ok(()) => source == DATA_SOURCE_INTERNAL_SPEAKER,
        Err(_) => true,
    }
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
/// Only true for unmuted, audible built-in speakers, headphones and
/// external devices can't leak regardless of volume or mute state.
fn can_leak(is_speakers: bool, muted: bool, volume: f32) -> bool {
    is_speakers && !muted && volume > AUDIBLE_VOLUME_THRESHOLD
}

/// After releasing a Bluetooth microphone, ask Core Audio to put the
/// headset back on A2DP stereo if it is still advertising HFP.
///
/// Disposing the input AudioUnit is what actually frees the SCO link; this
/// is a nudge for headsets (Bose in particular) that otherwise stay in HFP
/// until something else retoggles the output profile. No-op when the
/// default output is not Bluetooth, is already A2DP, or does not list
/// `a2dp` as a data source.
pub fn restore_bluetooth_a2dp() {
    let Ok(device) = default_output_device() else {
        return;
    };
    let mut transport: u32 = 0;
    if get_property(
        device,
        global_address(kAudioDevicePropertyTransportType),
        &mut transport,
    )
    .is_err()
        || (transport != kAudioDeviceTransportTypeBluetooth
            && transport != kAudioDeviceTransportTypeBluetoothLE)
    {
        return;
    }

    let mut source: u32 = 0;
    if get_property(
        device,
        output_address(kAudioDevicePropertyDataSource),
        &mut source,
    )
    .is_err()
        || source != DATA_SOURCE_HFP
    {
        return;
    }

    let sources = u32_property_list(device, output_address(kAudioDevicePropertyDataSources));
    if !sources.contains(&DATA_SOURCE_A2DP) {
        return;
    }

    if set_property(
        device,
        output_address(kAudioDevicePropertyDataSource),
        &DATA_SOURCE_A2DP,
    )
    .is_ok()
    {
        tracing::info!("Restored Bluetooth output from HFP to A2DP");
    }
}

fn set_property<T>(
    object: AudioObjectID,
    mut address: AudioObjectPropertyAddress,
    value: &T,
) -> Result<(), String> {
    let status = unsafe {
        AudioObjectSetPropertyData(
            object,
            NonNull::from(&mut address),
            0,
            std::ptr::null(),
            size_of::<T>() as u32,
            NonNull::new(value as *const T as *mut c_void).expect("non-null in pointer"),
        )
    };
    if status != 0 {
        return Err(format!(
            "AudioObjectSetPropertyData({:#x}) failed: {status}",
            address.mSelector
        ));
    }
    Ok(())
}

fn u32_property_list(object: AudioObjectID, mut address: AudioObjectPropertyAddress) -> Vec<u32> {
    let mut size: u32 = 0;
    let size_status = unsafe {
        AudioObjectGetPropertyDataSize(
            object,
            NonNull::from(&mut address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        )
    };
    if size_status != 0 || size == 0 {
        return Vec::new();
    }
    let count = size as usize / size_of::<u32>();
    let mut values = vec![0u32; count];
    let mut out_size = size;
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&mut address),
            0,
            std::ptr::null(),
            NonNull::from(&mut out_size),
            NonNull::new(values.as_mut_ptr() as *mut c_void).expect("non-null out pointer"),
        )
    };
    if status != 0 {
        return Vec::new();
    }
    values.truncate(out_size as usize / size_of::<u32>());
    values
}

/// Whether the default output can acoustically leak into the microphone
/// right now: built-in speakers, unmuted, and at an audible volume. This is
/// the only case where echo cancellation does useful work; running it
/// against a muted or silenced output starves the canceller of any real
/// echo to converge on and it ends up suppressing the mic instead.
pub fn output_can_leak_into_mic() -> bool {
    let Ok(device) = default_output_device() else {
        return false;
    };

    let is_speakers = output_is_builtin_speakers(device);
    if !is_speakers {
        return false;
    }

    can_leak(is_speakers, output_is_muted(device), output_volume(device))
}

#[cfg(test)]
mod tests {
    use super::can_leak;

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
}
