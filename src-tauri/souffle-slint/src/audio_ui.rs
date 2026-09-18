//! Audio + Microphone tabs (SOU-188 milestone 4). Device pickers forward
//! their chosen *label* back to Rust rather than a uid/index resolved in
//! `.slint` - see `microphone_section.slint`'s doc comment for why. Minute
//! dropdowns (autostop/max-duration) format their own labels from
//! `SettingsOptions::current()`'s values (AC3: no bounds redeclared here)
//! and parse them back deterministically, so no lookup table is needed on
//! either side of the round trip.

use crate::microphone_list::{self, MicrophoneListEntry};
use crate::{MainWindow, MicrophoneRow};
use souffle_lib::audio::AudioInputDevice;

const AUTOMATIC_LABEL: &str = "Automatique";
const CLAMSHELL_FOLLOW_LABEL: &str = "Suivre l'appareil par défaut";

fn device_label(device: &AudioInputDevice) -> String {
    if device.is_default {
        format!("{} (par défaut)", device.name)
    } else {
        device.name.clone()
    }
}

/// Pushes the two device pickers' label lists + current selection. `selected`
/// is the raw `audio_device` setting (may be empty for "automatic"),
/// `clamshell` is `clamshell_audio_device` (`None` for "follow default").
pub fn populate_device_pickers(
    window: &MainWindow,
    devices: &[AudioInputDevice],
    selected: &str,
    clamshell: Option<&str>,
) {
    let mut labels = vec![AUTOMATIC_LABEL.to_string()];
    labels.extend(devices.iter().map(device_label));
    let labels: Vec<slint::SharedString> = labels.into_iter().map(Into::into).collect();
    window.set_settings_audio_device_labels(std::rc::Rc::new(slint::VecModel::from(labels)).into());
    let selected_label = devices
        .iter()
        .find(|d| d.uid == selected)
        .map(device_label)
        .unwrap_or_else(|| AUTOMATIC_LABEL.to_string());
    window.set_settings_selected_device_label(selected_label.into());
    window.set_settings_pin_unavailable(
        !selected.is_empty() && !devices.iter().any(|d| d.uid == selected),
    );

    let mut clamshell_labels = vec![CLAMSHELL_FOLLOW_LABEL.to_string()];
    clamshell_labels.extend(devices.iter().map(device_label));
    let clamshell_labels: Vec<slint::SharedString> =
        clamshell_labels.into_iter().map(Into::into).collect();
    window.set_settings_clamshell_device_labels(
        std::rc::Rc::new(slint::VecModel::from(clamshell_labels)).into(),
    );
    let clamshell_label = clamshell
        .and_then(|uid| devices.iter().find(|d| d.uid == uid))
        .map(device_label)
        .unwrap_or_else(|| CLAMSHELL_FOLLOW_LABEL.to_string());
    window.set_settings_clamshell_device_label(clamshell_label.into());
}

/// Inverse of `populate_device_pickers`' label building - `None` means the
/// picker's "automatic"/"follow default" sentinel was chosen.
pub fn resolve_device_uid(devices: &[AudioInputDevice], label: &str) -> Option<String> {
    if label == AUTOMATIC_LABEL || label == CLAMSHELL_FOLLOW_LABEL {
        return None;
    }
    devices
        .iter()
        .find(|d| device_label(d) == label)
        .map(|d| d.uid.clone())
}

/// Port of `resolveSampleRateDeviceUid()`: the pin if connected, else the
/// default input, else the first device.
pub fn resolve_sample_rate_device_uid<'a>(
    selected: &str,
    devices: &'a [AudioInputDevice],
) -> Option<&'a str> {
    if !selected.is_empty() && devices.iter().any(|d| d.uid == selected) {
        return devices
            .iter()
            .find(|d| d.uid == selected)
            .map(|d| d.uid.as_str());
    }
    devices
        .iter()
        .find(|d| d.is_default)
        .or_else(|| devices.first())
        .map(|d| d.uid.as_str())
}

const CONFERENCING_SAMPLE_RATE_HZ: u32 = 48_000;

pub fn sample_rate_blocks_conferencing(hz: u32) -> bool {
    hz > CONFERENCING_SAMPLE_RATE_HZ
}

pub fn format_sample_rate_hz(hz: u32) -> String {
    if hz.is_multiple_of(1000) {
        format!("{} kHz", hz / 1000)
    } else if hz.is_multiple_of(100) {
        format!("{:.1} kHz", hz as f64 / 1000.0)
    } else {
        format!("{hz} Hz")
    }
}

/// Formats a minute count the same way in both directions of the round
/// trip: `minute_label` builds the dropdown text, `parse_minute_label`
/// recovers the value from it - no separate lookup table to keep in sync.
pub fn minute_label(value: u32) -> String {
    if value >= 60 && value.is_multiple_of(60) {
        format!("{} h", value / 60)
    } else {
        format!("{value} min")
    }
}

pub fn minute_labels(values: &[u32]) -> Vec<String> {
    values.iter().copied().map(minute_label).collect()
}

pub fn parse_minute_label(label: &str) -> Option<u32> {
    if let Some(h) = label.strip_suffix(" h") {
        h.trim().parse::<u32>().ok().map(|h| h * 60)
    } else {
        label
            .strip_suffix(" min")
            .and_then(|m| m.trim().parse::<u32>().ok())
    }
}

/// Relative "last seen" wording - not a byte-for-byte port of
/// `lastSeenAge()`'s pluralization/i18n, but the same information.
fn last_seen_label(last_seen_unix: i64) -> String {
    let now = chrono::Utc::now().timestamp();
    let diff = (now - last_seen_unix).max(0);
    if diff < 60 {
        "à l'instant".to_string()
    } else if diff < 3600 {
        format!("il y a {} min", diff / 60)
    } else if diff < 86_400 {
        format!("il y a {} h", diff / 3600)
    } else {
        format!("il y a {} j", diff / 86_400)
    }
}

pub fn populate_microphones(window: &MainWindow, list: &[MicrophoneListEntry]) {
    let has_disconnected = list.iter().any(|e| !e.connected);
    let count = list.len();
    let rows: Vec<MicrophoneRow> = list
        .iter()
        .enumerate()
        .map(|(index, entry)| MicrophoneRow {
            uid: entry.uid.as_str().into(),
            name: entry.name.as_str().into(),
            transport_label: microphone_list::transport_label(entry.transport).into(),
            is_default: entry.is_default,
            connected: entry.connected,
            hidden: entry.hidden,
            last_seen_label: entry
                .last_seen
                .map(last_seen_label)
                .unwrap_or_default()
                .into(),
            is_first: index == 0,
            is_last: index + 1 == count,
        })
        .collect();
    window.set_settings_microphones(std::rc::Rc::new(slint::VecModel::from(rows)).into());
    window.set_settings_has_disconnected_devices(has_disconnected);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minute_label_round_trips() {
        for value in [5, 10, 15, 30, 120, 240, 480] {
            let label = minute_label(value);
            assert_eq!(parse_minute_label(&label), Some(value), "label={label}");
        }
    }

    #[test]
    fn minute_label_uses_hours_above_an_even_hour() {
        assert_eq!(minute_label(120), "2 h");
        assert_eq!(minute_label(90), "90 min");
    }

    #[test]
    fn format_sample_rate_prefers_khz() {
        assert_eq!(format_sample_rate_hz(48_000), "48 kHz");
        assert_eq!(format_sample_rate_hz(44_100), "44.1 kHz");
        assert_eq!(format_sample_rate_hz(8_000), "8 kHz");
    }

    #[test]
    fn sample_rate_blocks_conferencing_above_48khz() {
        assert!(!sample_rate_blocks_conferencing(48_000));
        assert!(sample_rate_blocks_conferencing(96_000));
    }

    fn device(uid: &str, is_default: bool) -> AudioInputDevice {
        AudioInputDevice {
            uid: uid.to_string(),
            name: uid.to_string(),
            transport: souffle_lib::audio::TransportType::Usb,
            is_default,
        }
    }

    #[test]
    fn resolve_sample_rate_device_prefers_the_pin_when_connected() {
        let devices = vec![device("a", false), device("b", true)];
        assert_eq!(resolve_sample_rate_device_uid("a", &devices), Some("a"));
        assert_eq!(
            resolve_sample_rate_device_uid("missing", &devices),
            Some("b")
        );
        assert_eq!(resolve_sample_rate_device_uid("", &devices), Some("b"));
    }

    #[test]
    fn resolve_device_uid_returns_none_for_sentinels() {
        let devices = vec![device("a", false)];
        assert_eq!(resolve_device_uid(&devices, AUTOMATIC_LABEL), None);
        assert_eq!(resolve_device_uid(&devices, CLAMSHELL_FOLLOW_LABEL), None);
        assert_eq!(resolve_device_uid(&devices, "a"), Some("a".to_string()));
    }
}
