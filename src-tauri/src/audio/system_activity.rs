//! Passive system-audio activity probe for calendar auto-start nudges.
//!
//! While calendar integration is on and a meeting block is in progress, a
//! lightweight Core Audio process tap runs on a disposable thread and records
//! when output energy crosses a threshold. The calendar scheduler reads this
//! to avoid nudging before the user has actually joined a call.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tracing::warn;

use crate::platform;

/// RMS above this counts as "system audio active" (mixed output from calls).
pub const ACTIVITY_RMS_THRESHOLD: f32 = 0.006;

/// How long after the last above-threshold sample the leg stays "active".
pub const ACTIVITY_RECENCY: Duration = Duration::from_secs(90);

/// Shared timestamp updated by the probe thread.
#[derive(Debug, Default)]
pub struct SystemAudioActivity {
    /// Milliseconds since the UNIX epoch when activity was last seen; 0 = never.
    last_active_epoch_ms: AtomicU64,
}

impl SystemAudioActivity {
    pub fn mark_active(&self) {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.last_active_epoch_ms.store(ms, Ordering::Relaxed);
    }

    pub fn is_recently_active(&self, within: Duration) -> bool {
        let last = self.last_active_epoch_ms.load(Ordering::Relaxed);
        if last == 0 {
            return false;
        }
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        now_ms.saturating_sub(last) <= within.as_millis() as u64
    }
}

/// Handle to a background probe; dropping it tears the tap down.
pub struct SystemAudioProbe {
    _stop_tx: std::sync::mpsc::Sender<()>,
}

impl SystemAudioProbe {
    /// Start probing when supported. Returns `None` when taps are unavailable
    /// or tap creation fails (permission denied, CoreAudio wedged, etc.).
    pub fn start(activity: Arc<SystemAudioActivity>) -> Option<Self> {
        if !platform::system_audio_capture_supported() {
            return None;
        }

        #[cfg(target_os = "macos")]
        {
            use ringbuf::traits::Split;
            use ringbuf::HeapRb;

            let (prod, cons) = HeapRb::<f32>::new(48_000).split();
            let tap = match super::system_tap::spawn_tap(prod, Duration::from_secs(8)) {
                Ok(handle) => handle,
                Err(e) => {
                    warn!("System audio activity probe: tap failed: {e}");
                    return None;
                }
            };

            let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
            std::thread::Builder::new()
                .name("system-audio-probe".into())
                .spawn(move || run_probe_loop(cons, activity, stop_rx, tap))
                .ok()?;

            Some(Self { _stop_tx: stop_tx })
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = activity;
            None
        }
    }
}

#[cfg(target_os = "macos")]
fn run_probe_loop(
    mut cons: ringbuf::HeapCons<f32>,
    activity: Arc<SystemAudioActivity>,
    stop_rx: std::sync::mpsc::Receiver<()>,
    _tap: super::system_tap::TapHandle,
) {
    use ringbuf::traits::Consumer;

    let mut scratch = vec![0.0f32; 4096];
    loop {
        if stop_rx.try_recv().is_ok() {
            break;
        }
        let mut n = cons.pop_slice(&mut scratch);
        while n > 0 {
            if rms(&scratch[..n]) >= ACTIVITY_RMS_THRESHOLD {
                activity.mark_active();
            }
            n = cons.pop_slice(&mut scratch);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_of_silence_is_zero() {
        assert_eq!(rms(&[0.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn rms_detects_energy() {
        let samples = vec![0.1f32; 100];
        assert!(rms(&samples) >= ACTIVITY_RMS_THRESHOLD);
    }

    #[test]
    fn recency_tracks_mark_active() {
        let activity = SystemAudioActivity::default();
        assert!(!activity.is_recently_active(ACTIVITY_RECENCY));
        activity.mark_active();
        assert!(activity.is_recently_active(ACTIVITY_RECENCY));
    }
}
