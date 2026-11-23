//! Live input level of one microphone, for the onboarding microphone step.
//!
//! Opens a plain input stream, keeps only a normalized RMS and never records
//! or forwards a sample. The stream lives on its own thread so a slow
//! CoreAudio open cannot stall the UI thread; dropping the probe stops it.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;
use tracing::{debug, warn};

use super::capture::AudioCapture;

const STOP_POLL: Duration = Duration::from_millis(50);

pub struct LevelProbe {
    level: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
}

impl LevelProbe {
    /// Probe the device with this UID. An empty or unknown UID falls back to
    /// the OS default input, like a recording does.
    pub fn start(uid: &str) -> Self {
        let level = Arc::new(AtomicU32::new(0f32.to_bits()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_level = Arc::clone(&level);
        let thread_stop = Arc::clone(&stop);
        let uid = uid.to_string();
        let spawned = std::thread::Builder::new()
            .name("mic-level-probe".into())
            .spawn(move || run(&uid, &thread_level, &thread_stop));
        if let Err(error) = spawned {
            warn!("Could not start the microphone level probe: {error}");
        }
        Self { level, stop }
    }

    /// Shared normalized level (0 to 1, stored as `f32` bits), readable after
    /// the probe itself has been dropped.
    pub fn reader(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.level)
    }
}

impl Drop for LevelProbe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn run(uid: &str, level: &Arc<AtomicU32>, stop: &AtomicBool) {
    let Some(stream) = open(uid, level) else {
        return;
    };
    if stop.load(Ordering::Relaxed) {
        return;
    }
    if let Err(error) = stream.play() {
        warn!("Microphone level probe failed to start: {error}");
        return;
    }
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(STOP_POLL);
    }
    drop(stream);
    level.store(0f32.to_bits(), Ordering::Relaxed);
}

fn open(uid: &str, level: &Arc<AtomicU32>) -> Option<Stream> {
    let host = cpal::default_host();
    let device = select_device(&host, uid)?;
    let config = AudioCapture::preferred_config(&device)
        .inspect_err(|error| debug!("Microphone level probe has no config: {error}"))
        .ok()?;
    let callback_level = Arc::clone(level);
    device
        .build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if data.is_empty() {
                        return;
                    }
                    let sum_sq: f32 = data.iter().map(|s| s * s).sum();
                    let rms = (sum_sq / data.len() as f32).sqrt();
                    callback_level.store((rms * 8.0).min(1.0).to_bits(), Ordering::Relaxed);
                }));
            },
            |error| warn!("Microphone level probe stream error: {error}"),
            None,
        )
        .inspect_err(|error| warn!("Microphone level probe failed to open: {error}"))
        .ok()
}

fn select_device(host: &cpal::Host, uid: &str) -> Option<Device> {
    if !uid.is_empty()
        && let Some(device) = super::capture::find_cpal_device_by_uid(host, uid)
    {
        return Some(device);
    }
    host.default_input_device()
}
