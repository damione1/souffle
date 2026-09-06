//! Nominal sample-rate read/repair for a CoreAudio input device.
//!
//! Capture start never writes `kAudioDevicePropertyNominalSampleRate` when the
//! device is already in range (see `choose_input_sample_rate`). This module is
//! the opt-in repair path: Settings shows the current rate and writes 48 kHz
//! only after an explicit click.

/// Rate conferencing apps (Teams, Zoom) can open. Also the capture fallback
/// when the device's current rate can't be read.
pub const CONFERENCING_SAMPLE_RATE: u32 = 48_000;

/// Pick a repair target inside a single supported `[min, max]` range.
///
/// Prefer 48 kHz when it fits. If the whole range is below 48 kHz, take the
/// highest supported rate. If the whole range is above 48 kHz, clamp 48 kHz
/// into the range (the lowest supported rate).
pub fn choose_repair_sample_rate(min: u32, max: u32) -> u32 {
    if (min..=max).contains(&CONFERENCING_SAMPLE_RATE) {
        CONFERENCING_SAMPLE_RATE
    } else if max < CONFERENCING_SAMPLE_RATE {
        max
    } else {
        CONFERENCING_SAMPLE_RATE.clamp(min, max)
    }
}

/// Same policy over CoreAudio `AudioValueRange`s (discrete points or spans).
///
/// Empty `ranges` falls back to 48 kHz. The HAL write will fail if the
/// device truly cannot do it.
pub fn choose_repair_sample_rate_from_ranges(ranges: &[(u32, u32)]) -> u32 {
    if ranges.is_empty() {
        return CONFERENCING_SAMPLE_RATE;
    }
    if ranges
        .iter()
        .any(|(min, max)| (*min..=*max).contains(&CONFERENCING_SAMPLE_RATE))
    {
        return CONFERENCING_SAMPLE_RATE;
    }
    if let Some(rate) = ranges
        .iter()
        .filter_map(|(min, max)| {
            (*min <= CONFERENCING_SAMPLE_RATE).then_some((*max).min(CONFERENCING_SAMPLE_RATE))
        })
        .max()
    {
        return rate;
    }
    let min = ranges
        .iter()
        .map(|(min, _)| *min)
        .min()
        .unwrap_or(CONFERENCING_SAMPLE_RATE);
    let max = ranges
        .iter()
        .map(|(_, max)| *max)
        .max()
        .unwrap_or(CONFERENCING_SAMPLE_RATE);
    CONFERENCING_SAMPLE_RATE.clamp(min, max)
}

/// Current `kAudioDevicePropertyNominalSampleRate` for `device_uid`.
/// An empty UID uses the system default input.
pub fn read_input_sample_rate(device_uid: &str) -> Result<u32, String> {
    #[cfg(target_os = "macos")]
    {
        macos::read_input_sample_rate(device_uid)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = device_uid;
        Err("Reading the microphone sample rate is only supported on macOS".into())
    }
}

/// Write a conferencing-safe nominal rate for `device_uid`, once.
/// An empty UID uses the system default input.
pub fn reset_input_sample_rate(device_uid: &str) -> Result<u32, String> {
    #[cfg(target_os = "macos")]
    {
        macos::reset_input_sample_rate(device_uid)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = device_uid;
        Err("Resetting the microphone sample rate is only supported on macOS".into())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;
    use std::ptr::NonNull;

    use objc2_core_audio::{
        AudioObjectGetPropertyData, AudioObjectHasProperty, AudioObjectID,
        AudioObjectPropertyAddress, AudioObjectSetPropertyData,
        kAudioDevicePropertyAvailableNominalSampleRates, kAudioDevicePropertyNominalSampleRate,
    };
    use objc2_core_audio_types::AudioValueRange;
    use tracing::{info, warn};

    use super::choose_repair_sample_rate_from_ranges;
    use crate::audio::device_watch::{
        default_input_device_id, get_property, global_address, object_id_for_uid,
        property_data_size,
    };

    /// Bounded wait for the HAL to apply a nominal-rate write.
    const SETTLE_ATTEMPTS: u32 = 20;
    const SETTLE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

    fn resolve_device(device_uid: &str) -> Result<AudioObjectID, String> {
        if device_uid.is_empty() {
            default_input_device_id().ok_or_else(|| "No default input device".into())
        } else {
            object_id_for_uid(device_uid)
                .ok_or_else(|| format!("Microphone not found: {device_uid}"))
        }
    }

    fn hz_from_f64(rate: f64) -> u32 {
        rate.round().clamp(1.0, u32::MAX as f64) as u32
    }

    fn has_property(object: AudioObjectID, mut address: AudioObjectPropertyAddress) -> bool {
        unsafe { AudioObjectHasProperty(object, NonNull::from(&mut address)) }
    }

    fn nominal_rate(device: AudioObjectID) -> Result<u32, String> {
        let address = global_address(kAudioDevicePropertyNominalSampleRate);
        if !has_property(device, address) {
            return Err("This microphone does not report a sample rate".into());
        }
        let mut rate = 0.0f64;
        if !get_property(device, address, &mut rate) || !rate.is_finite() || rate <= 0.0 {
            return Err("Could not read this microphone's sample rate".into());
        }
        Ok(hz_from_f64(rate))
    }

    fn available_rates(device: AudioObjectID) -> Vec<(u32, u32)> {
        let address = global_address(kAudioDevicePropertyAvailableNominalSampleRates);
        if !has_property(device, address) {
            return Vec::new();
        }
        let size = property_data_size(device, address);
        let count = size as usize / size_of::<AudioValueRange>();
        if count == 0 {
            return Vec::new();
        }
        let mut ranges = vec![
            AudioValueRange {
                mMinimum: 0.0,
                mMaximum: 0.0,
            };
            count
        ];
        let mut out_size = size;
        let mut addr = address;
        let status = unsafe {
            AudioObjectGetPropertyData(
                device,
                NonNull::from(&mut addr),
                0,
                std::ptr::null(),
                NonNull::from(&mut out_size),
                NonNull::new(ranges.as_mut_ptr() as *mut c_void).expect("non-null out pointer"),
            )
        };
        if status != 0 {
            return Vec::new();
        }
        let n = out_size as usize / size_of::<AudioValueRange>();
        ranges.truncate(n);
        ranges
            .into_iter()
            .filter(|range| {
                range.mMinimum.is_finite()
                    && range.mMaximum.is_finite()
                    && range.mMaximum >= range.mMinimum
                    && range.mMaximum > 0.0
            })
            .map(|range| (hz_from_f64(range.mMinimum), hz_from_f64(range.mMaximum)))
            .collect()
    }

    fn set_nominal_rate(device: AudioObjectID, rate_hz: u32) -> Result<(), String> {
        let mut address = global_address(kAudioDevicePropertyNominalSampleRate);
        if !has_property(device, address) {
            return Err(
                "This microphone does not expose a sample rate that Souffle can change".into(),
            );
        }
        let value = f64::from(rate_hz);
        let status = unsafe {
            AudioObjectSetPropertyData(
                device,
                NonNull::from(&mut address),
                0,
                std::ptr::null(),
                size_of::<f64>() as u32,
                NonNull::new((&value as *const f64).cast::<c_void>().cast_mut())
                    .expect("non-null rate pointer"),
            )
        };
        if status != 0 {
            return Err(format!(
                "Could not set the sample rate to {rate_hz} Hz (CoreAudio status {status}). \
                 Try Audio MIDI Setup."
            ));
        }
        Ok(())
    }

    pub(super) fn read_input_sample_rate(device_uid: &str) -> Result<u32, String> {
        nominal_rate(resolve_device(device_uid)?)
    }

    pub(super) fn reset_input_sample_rate(device_uid: &str) -> Result<u32, String> {
        let device = resolve_device(device_uid)?;
        let current = nominal_rate(device).ok();
        let target = choose_repair_sample_rate_from_ranges(&available_rates(device));

        info!(
            uid = device_uid,
            from = current,
            to = target,
            "Reset input sample rate on user request"
        );
        set_nominal_rate(device, target)?;
        Ok(settled_rate(device, target).unwrap_or_else(|| {
            warn!(
                uid = device_uid,
                wanted = target,
                "Nominal sample rate never settled on the requested rate"
            );
            target
        }))
    }

    /// The HAL applies the write asynchronously, so an immediate read can still
    /// return the old rate. Poll until it settles, then report what stuck.
    fn settled_rate(device: AudioObjectID, target: u32) -> Option<u32> {
        let mut last = None;
        for _ in 0..SETTLE_ATTEMPTS {
            match nominal_rate(device) {
                Ok(actual) if actual == target => return Some(actual),
                Ok(actual) => last = Some(actual),
                Err(_) => {}
            }
            std::thread::sleep(SETTLE_INTERVAL);
        }
        last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_picks_48k_when_the_range_includes_it() {
        assert_eq!(choose_repair_sample_rate(8_000, 96_000), 48_000);
        assert_eq!(choose_repair_sample_rate(48_000, 48_000), 48_000);
        assert_eq!(choose_repair_sample_rate(16_000, 48_000), 48_000);
    }

    #[test]
    fn repair_clamps_to_the_highest_rate_at_or_below_48k() {
        assert_eq!(choose_repair_sample_rate(8_000, 44_100), 44_100);
        assert_eq!(choose_repair_sample_rate(16_000, 16_000), 16_000);
    }

    #[test]
    fn repair_clamps_fallback_when_the_range_is_entirely_above_48k() {
        assert_eq!(choose_repair_sample_rate(88_200, 192_000), 88_200);
        assert_eq!(choose_repair_sample_rate(96_000, 96_000), 96_000);
    }

    #[test]
    fn repair_from_ranges_prefers_48k_when_any_range_covers_it() {
        assert_eq!(
            choose_repair_sample_rate_from_ranges(&[(8_000, 44_100), (48_000, 96_000)]),
            48_000
        );
        assert_eq!(
            choose_repair_sample_rate_from_ranges(&[(48_000, 48_000), (96_000, 96_000)]),
            48_000
        );
    }

    #[test]
    fn repair_from_ranges_picks_highest_discrete_rate_at_or_below_48k() {
        assert_eq!(
            choose_repair_sample_rate_from_ranges(&[(44_100, 44_100), (96_000, 96_000)]),
            44_100
        );
        assert_eq!(
            choose_repair_sample_rate_from_ranges(&[(8_000, 44_100), (88_200, 96_000)]),
            44_100
        );
    }

    #[test]
    fn repair_from_ranges_falls_back_when_nothing_is_at_or_below_48k() {
        assert_eq!(
            choose_repair_sample_rate_from_ranges(&[(88_200, 88_200), (96_000, 96_000)]),
            88_200
        );
    }

    #[test]
    fn repair_from_empty_ranges_uses_48k() {
        assert_eq!(choose_repair_sample_rate_from_ranges(&[]), 48_000);
    }
}
