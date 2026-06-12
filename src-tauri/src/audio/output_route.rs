//! Default-output-device introspection for system-audio capture.
//!
//! The system tap's aggregate device must reference the current default
//! output device, and echo cancellation is only useful when that output is
//! the built-in speakers (headphones physically can't leak into the mic).

#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2_core_audio::{
    AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress,
    kAudioDevicePropertyDataSource, kAudioDevicePropertyDeviceUID,
    kAudioDevicePropertyTransportType, kAudioDeviceTransportTypeBuiltIn,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject,
};
use objc2_core_foundation::{CFRetained, CFString};

/// Built-in output data source: internal speaker ('ispk').
const DATA_SOURCE_INTERNAL_SPEAKER: u32 = 0x6973_706b;

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

/// Whether the default output routes to the built-in speakers — the only
/// case where system audio can acoustically leak back into the microphone
/// and echo cancellation is worth running.
pub fn output_is_builtin_speakers() -> bool {
    let Ok(device) = default_output_device() else {
        return false;
    };

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

    // Built-in transport covers both the speakers and the headphone jack;
    // the data source ('ispk' vs 'hdpn') tells them apart. If the device
    // doesn't report one, assume speakers.
    let mut source: u32 = 0;
    let address = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyDataSource,
        mScope: kAudioObjectPropertyScopeOutput,
        mElement: kAudioObjectPropertyElementMain,
    };
    match get_property(device, address, &mut source) {
        Ok(()) => source == DATA_SOURCE_INTERNAL_SPEAKER,
        Err(_) => true,
    }
}
