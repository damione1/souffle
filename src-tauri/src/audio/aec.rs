//! Acoustic echo cancellation for meeting mode.
//!
//! When the user plays meeting audio through the built-in speakers, the
//! other participants' voices re-enter the microphone and would be
//! transcribed twice. The system-audio tap gives us the exact far-end
//! (render) signal, so a WebRTC-style AEC can subtract it from the mic.
//!
//! Thin wrapper around `sonora` (pure-Rust port of the WebRTC audio
//! processing module) so the backend can be swapped without touching the
//! mixer. Works on 10ms mono frames at the mixer rate.

use sonora::config::EchoCanceller;
use sonora::{AudioProcessing, Config, StreamConfig};

/// Coarse estimate of the delay between a render frame reaching sonora and
/// the corresponding echo showing up in a capture frame, passed to
/// `Aec::set_expected_delay_ms` by every real caller. This is not a measured
/// value (macOS doesn't expose one cheaply from the mixer's position, and the
/// combined output+input CoreAudio buffering plus acoustic propagation for
/// built-in speakers/mic commonly falls in the tens of ms), just a mid-range
/// heuristic for AEC3's own delay estimator to start searching from; see
/// `set_expected_delay_ms`'s docs for why an approximate hint still helps.
pub const EXPECTED_ACOUSTIC_DELAY_MS: i32 = 50;

/// How many times a panicking sonora call may recreate a fresh instance
/// before echo cancellation gives up for the rest of the session. Each
/// rearm loses whatever echo-path convergence sonora had built up, so this
/// trades a few seconds of reduced cancellation while it reconverges for
/// not falling back to raw mic for the rest of a (possibly hour-long)
/// meeting. Three gives real transient bugs (the kind seen in production:
/// one panic on an unusual signal transition) room to recover without
/// letting a pathological, repeatedly-panicking input spin forever.
const MAX_REARM_ATTEMPTS: u32 = 3;

/// What a panic during a sonora call should do next, given how many times
/// this session has already rearmed. Pure decision function, kept free of
/// `Aec`'s state so it can be unit-tested directly (mirrors `decide_mic_loss`
/// in `capture.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RearmDecision {
    /// Recreate the sonora instance from scratch and keep going.
    Rearm,
    /// Give up for the rest of the session.
    GiveUp,
}

fn decide_rearm(attempts_so_far: u32) -> RearmDecision {
    if attempts_so_far < MAX_REARM_ATTEMPTS {
        RearmDecision::Rearm
    } else {
        RearmDecision::GiveUp
    }
}

fn build_apm(sample_rate: u32) -> AudioProcessing {
    let stream = StreamConfig::new(sample_rate, 1);
    AudioProcessing::builder()
        .config(Config {
            echo_canceller: Some(EchoCanceller::default()),
            ..Config::default()
        })
        .capture_config(stream)
        .render_config(stream)
        .build()
}

pub struct Aec {
    apm: AudioProcessing,
    sample_rate: u32,
    /// 10ms at the configured sample rate.
    frame_len: usize,
    render_out: Vec<f32>,
    /// Hint passed to sonora's `set_stream_delay_ms` (the expected delay
    /// between a render frame being handed to sonora and the corresponding
    /// echo showing up in a capture frame). Reapplied on every rearm since a
    /// fresh sonora instance starts with no delay set. `None` until
    /// `set_expected_delay_ms` is called, matching sonora's own default.
    expected_delay_ms: Option<i32>,
    /// Panics recovered so far this session by recreating sonora fresh (see
    /// `MAX_REARM_ATTEMPTS`).
    rearm_count: u32,
    /// Set once rearming is exhausted. Echo cancellation is a non-essential
    /// enhancement, so once it can't recover we stop calling it and let the
    /// mic pass through uncancelled rather than risk taking down the session.
    disabled: bool,
}

impl Aec {
    pub fn new(sample_rate: u32) -> Self {
        let frame_len = sample_rate as usize / 100;
        Self {
            apm: build_apm(sample_rate),
            sample_rate,
            frame_len,
            render_out: vec![0.0; frame_len],
            expected_delay_ms: None,
            rearm_count: 0,
            disabled: false,
        }
    }

    /// Builds an `Aec` with `EXPECTED_ACOUSTIC_DELAY_MS` applied. What every
    /// real caller wants; `new` stays hint-free so tests (and the mixer
    /// bench) can compare with and without it.
    pub fn new_with_default_delay_hint(sample_rate: u32) -> Self {
        let mut aec = Self::new(sample_rate);
        aec.set_expected_delay_ms(EXPECTED_ACOUSTIC_DELAY_MS);
        aec
    }

    /// Hints the expected delay between render and capture (see sonora's
    /// `set_stream_delay_ms` docs). AEC3's own delay estimator does not
    /// strictly require this (unlike the legacy AEC2 the doc comment is
    /// inherited from), but an external estimate narrows its search and
    /// speeds up convergence. Applied immediately and reapplied on every
    /// rearm, since a freshly built sonora instance starts with no hint.
    pub fn set_expected_delay_ms(&mut self, delay_ms: i32) {
        self.expected_delay_ms = Some(delay_ms);
        self.apply_delay_hint();
    }

    fn apply_delay_hint(&mut self) {
        if let Some(delay_ms) = self.expected_delay_ms {
            let _ = self.apm.set_stream_delay_ms(delay_ms);
        }
    }

    /// Feed a 10ms far-end frame (the system audio about to leave the
    /// speakers). Must be called before the capture frame of the same tick.
    pub fn process_render(&mut self, frame: &[f32]) {
        debug_assert_eq!(frame.len(), self.frame_len);
        if self.disabled {
            return;
        }
        let apm = &mut self.apm;
        let render_out = &mut self.render_out;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = apm.process_render_f32(&[frame], &mut [&mut render_out[..]]);
        }));
        if result.is_err() {
            self.handle_panic();
        }
    }

    /// Remove the echo of previously-rendered audio from a 10ms mic frame,
    /// in place.
    pub fn process_capture(&mut self, frame: &mut [f32]) {
        debug_assert_eq!(frame.len(), self.frame_len);
        if self.disabled {
            return;
        }
        let mut out = vec![0.0; self.frame_len];
        let apm = &mut self.apm;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            apm.process_capture_f32(&[&frame[..]], &mut [&mut out[..]])
                .is_ok()
        }));
        match result {
            Ok(true) => frame.copy_from_slice(&out),
            Ok(false) => {}
            Err(_) => self.handle_panic(),
        }
    }

    /// Recreate sonora from scratch (losing convergence state, which is
    /// inherent and acceptable) up to `MAX_REARM_ATTEMPTS` times, then fall
    /// back to disabled for the rest of the session.
    fn handle_panic(&mut self) {
        match decide_rearm(self.rearm_count) {
            RearmDecision::Rearm => {
                self.rearm_count += 1;
                tracing::warn!(
                    "Echo cancellation panicked; recreating it fresh (attempt {}/{MAX_REARM_ATTEMPTS}). \
                     Convergence is lost and will take a few seconds to rebuild.",
                    self.rearm_count
                );
                self.apm = build_apm(self.sample_rate);
                self.apply_delay_hint();
            }
            RearmDecision::GiveUp => {
                self.disabled = true;
                tracing::error!(
                    "Echo cancellation panicked again after {MAX_REARM_ATTEMPTS} rearms this \
                     session; disabling it for the rest of the session (mic passes through \
                     uncancelled)"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The far-end signal leaking into the mic with a small delay must come
    /// out attenuated once the canceller has converged.
    #[test]
    fn attenuates_far_end_leakage() {
        let rate = 48_000u32;
        let frame = rate as usize / 100;
        let mut aec = Aec::new(rate);

        // 2s of a 440Hz tone as far-end; mic hears it scaled, delayed 30ms.
        let tone: Vec<f32> = (0..rate as usize * 2)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.5)
            .collect();
        let delay = rate as usize * 30 / 1000;

        let mut leaked_energy_early = 0.0f32;
        let mut leaked_energy_late = 0.0f32;
        let total_frames = tone.len() / frame;
        for f in 0..total_frames {
            let start = f * frame;
            aec.process_render(&tone[start..start + frame]);

            let mut mic: Vec<f32> = (0..frame)
                .map(|i| {
                    let idx = start + i;
                    if idx >= delay {
                        tone[idx - delay] * 0.3
                    } else {
                        0.0
                    }
                })
                .collect();
            aec.process_capture(&mut mic);

            let energy: f32 = mic.iter().map(|s| s * s).sum();
            if f < total_frames / 4 {
                leaked_energy_early += energy;
            } else if f >= total_frames * 3 / 4 {
                leaked_energy_late += energy;
            }
        }

        assert!(
            leaked_energy_late < leaked_energy_early * 0.2,
            "echo should converge: early={leaked_energy_early}, late={leaked_energy_late}"
        );
    }

    #[test]
    fn decide_rearm_allows_up_to_the_cap_then_gives_up() {
        for n in 0..MAX_REARM_ATTEMPTS {
            assert_eq!(decide_rearm(n), RearmDecision::Rearm, "attempt {n} should rearm");
        }
        assert_eq!(decide_rearm(MAX_REARM_ATTEMPTS), RearmDecision::GiveUp);
        assert_eq!(decide_rearm(MAX_REARM_ATTEMPTS + 5), RearmDecision::GiveUp);
    }

    /// Repeated panics recreate sonora up to the cap, then disable it. This
    /// drives the same `handle_panic` path a real sonora panic would, without
    /// needing to reproduce the underlying crate bug.
    #[test]
    fn rearms_up_to_the_cap_then_disables() {
        let mut aec = Aec::new(48_000);

        for expected_attempt in 1..=MAX_REARM_ATTEMPTS {
            aec.handle_panic();
            assert_eq!(aec.rearm_count, expected_attempt);
            assert!(!aec.disabled, "should still be rearming at attempt {expected_attempt}");
        }

        // One panic more than the cap allows: gives up for the session.
        aec.handle_panic();
        assert!(aec.disabled);
    }

    #[test]
    fn process_calls_are_no_ops_once_disabled() {
        let mut aec = Aec::new(48_000);
        aec.disabled = true;

        let frame = vec![0.5; aec.frame_len];
        let mut mic = frame.clone();
        aec.process_render(&frame);
        aec.process_capture(&mut mic);

        assert_eq!(mic, frame, "a disabled Aec must leave the mic frame untouched");
    }

    /// A fresh sonora instance starts with no delay hint, so a rearm must
    /// reapply it or convergence speed regresses back to baseline on every
    /// recovered panic.
    #[test]
    fn delay_hint_survives_a_rearm() {
        let mut aec = Aec::new(48_000);
        aec.set_expected_delay_ms(42);
        assert_eq!(aec.apm.stream_delay_ms(), 42);

        aec.handle_panic();

        assert_eq!(
            aec.apm.stream_delay_ms(),
            42,
            "the delay hint must be reapplied to the rearmed instance"
        );
    }
}
