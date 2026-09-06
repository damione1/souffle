use tauri::State;

use crate::audio::capture::list_input_devices;
use crate::audio::route_notice::observe_and_notices;
use crate::audio::sample_rate;
use crate::audio::{AudioInputDevice, InputPriority, ResolveInputParams, resolve_input};
use crate::db::Database;
use crate::settings::AppSettings;
use crate::state::{AppState, AudioCommand};
use crossbeam_channel::Sender;
use tauri::AppHandle;
use tauri_specta::Event;

/// List available audio input devices
#[tauri::command]
#[specta::specta]
pub fn list_audio_devices() -> Result<Vec<AudioInputDevice>, String> {
    Ok(list_input_devices())
}

/// Select an audio input device by stable CoreAudio UID. When the UID is not
/// connected, the pin is kept and capture falls back through the priority
/// policy; the frontend is notified via [`crate::app_events::InputPinUnavailable`].
#[tauri::command]
#[specta::specta]
pub fn select_audio_device(
    app: AppHandle,
    state: State<'_, AppState>,
    device_uid: String,
) -> Result<(), String> {
    state
        .audio_cmd_sender
        .send(AudioCommand::SelectDevice(device_uid.clone()))
        .map_err(|e| format!("Failed to send device selection: {e}"))?;
    emit_pin_status(&app, device_uid.as_str());
    Ok(())
}

/// Store the current device list so the first real CoreAudio change toasts
/// instead of being treated as boot.
pub fn prime_input_route_snapshot(db: &Database) {
    let Ok((priority, allow_bluetooth_mic)) = AppSettings::sync_input_priority_from_devices(db)
    else {
        return;
    };
    let devices = list_input_devices();
    let settings = AppSettings::load(db).ok();
    let resolved =
        resolved_capture_uid(&devices, settings.as_ref(), &priority, allow_bluetooth_mic);
    let _ = observe_and_notices(&devices, resolved);
}

/// React to CoreAudio device-list or default-input changes: refresh known
/// devices, push the updated policy to capture, hot-swap when recording, and
/// notify the frontend.
pub fn handle_input_route_change(
    db: &Database,
    cmd_tx: &Sender<AudioCommand>,
    app: &AppHandle,
) -> Result<(), String> {
    let (priority, allow_bluetooth_mic) = AppSettings::sync_input_priority_from_devices(db)?;
    cmd_tx
        .send(AudioCommand::SetInputPolicy {
            priority: priority.clone(),
            allow_bluetooth_mic,
        })
        .map_err(|e| format!("Failed to push input policy: {e}"))?;

    let devices = list_input_devices();
    let _ = crate::app_events::InputDevicesChanged {
        devices: devices.clone(),
    }
    .emit(app);

    let settings = AppSettings::load(db).ok();
    let resolved =
        resolved_capture_uid(&devices, settings.as_ref(), &priority, allow_bluetooth_mic);
    for notice in observe_and_notices(&devices, resolved) {
        let _ = notice.emit(app);
    }

    if let Some(uid) = settings.as_ref().and_then(|s| s.audio_device.as_deref()) {
        emit_pin_status(app, uid);
    }
    Ok(())
}

fn resolved_capture_uid(
    devices: &[AudioInputDevice],
    settings: Option<&AppSettings>,
    priority: &InputPriority,
    allow_bluetooth_mic: bool,
) -> Option<String> {
    let pin = settings.and_then(|s| s.audio_device.as_deref());
    let clamshell_pref = settings.and_then(|s| s.clamshell_audio_device.as_deref());
    let clamshell_active =
        pin.is_none() && clamshell_pref.is_some() && crate::power::is_clamshell();
    resolve_input(
        devices,
        ResolveInputParams {
            pin,
            clamshell_pref,
            clamshell_active,
            priority,
            allow_bluetooth_mic,
        },
    )
}

fn emit_pin_status(app: &AppHandle, uid: &str) {
    if uid.is_empty() {
        return;
    }
    let connected = list_input_devices().iter().any(|device| device.uid == uid);
    if connected {
        let _ = crate::app_events::InputPinAvailable {
            uid: uid.to_string(),
        }
        .emit(app);
    } else {
        let _ = crate::app_events::InputPinUnavailable {
            uid: uid.to_string(),
        }
        .emit(app);
    }
}

/// Current `kAudioDevicePropertyNominalSampleRate` for an input device.
/// An empty UID uses the system default input. Read-only — never writes.
#[tauri::command]
#[specta::specta]
pub fn get_input_sample_rate(device_uid: String) -> Result<u32, String> {
    sample_rate::read_input_sample_rate(&device_uid)
}

/// Set the device's nominal sample rate to 48 kHz (or the closest supported
/// rate at or below it). Writes `kAudioDevicePropertyNominalSampleRate` once.
/// Call only from an explicit Settings click.
#[tauri::command]
#[specta::specta]
pub fn reset_input_sample_rate(device_uid: String) -> Result<u32, String> {
    sample_rate::reset_input_sample_rate(&device_uid)
}

/// Whether system-audio capture (Core Audio process taps) is available on this OS
#[tauri::command]
#[specta::specta]
pub fn get_system_audio_support() -> bool {
    crate::platform::system_audio_capture_supported()
}

/// Whether this Mac has a battery (i.e. is a laptop). Gates the
/// clamshell-microphone setting in the UI — meaningless on a desktop Mac.
#[tauri::command]
#[specta::specta]
pub fn is_laptop() -> bool {
    crate::power::is_laptop()
}

/// Debug: record system audio for `seconds` and write it to a WAV file.
/// Returns the file path. Exercises the tap end-to-end (TCC prompt included).
#[tauri::command]
#[specta::specta]
pub async fn debug_record_system_audio(seconds: u32) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || record_system_audio_wav(seconds))
        .await
        .map_err(|e| format!("Task failed: {e}"))?
}

#[cfg(target_os = "macos")]
fn record_system_audio_wav(seconds: u32) -> Result<String, String> {
    use ringbuf::HeapRb;
    use ringbuf::traits::{Consumer, Split};

    use crate::audio::system_tap::spawn_tap;

    let seconds = seconds.clamp(1, 60);
    // 1s of headroom at 48kHz; the drain loop below empties it every 50ms.
    let (producer, mut consumer) = HeapRb::<f32>::new(48_000 * 2).split();
    // Through spawn_tap rather than SystemTap::start directly: a wedged
    // coreaudiod must time out instead of parking this worker indefinitely,
    // and this path gets the orphan sweep like every other tap.
    let tap = spawn_tap(producer, std::time::Duration::from_secs(8))?;
    let sample_rate = tap.sample_rate;

    let mut samples: Vec<f32> = Vec::with_capacity(sample_rate as usize * seconds as usize);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds as u64);
    let mut chunk = vec![0f32; 4800];
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
        loop {
            let n = consumer.pop_slice(&mut chunk);
            samples.extend_from_slice(&chunk[..n]);
            if n < chunk.len() {
                break;
            }
        }
    }
    drop(tap);

    let path = crate::constants::app_data_dir().join(format!(
        "system_audio_debug_{}.wav",
        chrono::Utc::now().format("%Y%m%d_%H%M%S")
    ));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer =
        hound::WavWriter::create(&path, spec).map_err(|e| format!("WAV create failed: {e}"))?;
    for s in &samples {
        writer
            .write_sample(*s)
            .map_err(|e| format!("WAV write failed: {e}"))?;
    }
    writer
        .finalize()
        .map_err(|e| format!("WAV finalize failed: {e}"))?;

    tracing::info!(
        "Recorded {} system-audio samples to {}",
        samples.len(),
        path.display()
    );
    Ok(path.display().to_string())
}

#[cfg(not(target_os = "macos"))]
fn record_system_audio_wav(_seconds: u32) -> Result<String, String> {
    Err("System audio capture is only supported on macOS".into())
}
