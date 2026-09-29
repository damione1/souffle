//! Acoustic echo cancellation for meeting mode.
//!
//! When the user plays meeting audio through the built-in speakers the other
//! participants' voices re-enter the microphone and would be transcribed twice
//! (once on the "Them" lane captured from the tap, once on the "Me" lane
//! captured from the mic). The system-audio tap gives us the exact far-end
//! (render) signal, so an adaptive filter can subtract it from the mic.
//!
//! ## Why not WebRTC AEC (sonora)?
//!
//! The original implementation used `sonora`, a Rust port of the WebRTC audio
//! processing module. Its non-linear post-filter (NLP) is tuned for telephony,
//! where swallowing a word is preferred over letting an echo through. For an
//! ASR pipeline the trade-off is reversed: a residual echo produces a duplicate
//! that a text filter can handle, but a suppressed word is unrecoverable.
//! Measurement showed the NLP stage left the output **-10 dB** worse than raw
//! mic in double-talk (SOU-063 bench, 2026-07-18).
//!
//! ## Current approach: linear FDAF
//!
//! We use `fdaf-aec`, a pure-Rust Frequency Domain Adaptive Filter (FDAF) with
//! the Overlap-Save method. It performs only linear echo subtraction — no
//! non-linear suppression, no AGC, no noise reduction. The far-end signal is
//! pre-delayed by [`EXPECTED_ACOUSTIC_DELAY_MS`] before being fed to the
//! filter, aligning it with the echo that the mic will hear. Bench result:
//! double-talk ERLE **+19.7 dB**, echo-only ERLE **+30.8 dB**.

use fdaf_aec::FdafAec;
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Observer, Producer, Split};

/// Coarse estimate of the delay between a render frame leaving the speakers
/// and the corresponding echo arriving at the microphone. Used to pre-delay
/// the render buffer fed to the FDAF filter so both signals are aligned before
/// the filter tries to subtract one from the other. An unaligned reference is
/// the single most common cause of FDAF divergence.
pub const EXPECTED_ACOUSTIC_DELAY_MS: i32 = 50;

// fdaf-aec requires fft_size to be a power of two; frame_size is fft_size / 2.
// At 48 kHz, FFT_SIZE = 4096 covers 85 ms of filter length, more than enough
// to cancel the 50 ms acoustic delay plus typical room reverb tail.
const FFT_SIZE: usize = 4096;
const FRAME_SIZE: usize = FFT_SIZE / 2; // 2048 samples ≈ 42.7 ms at 48 kHz

/// Acoustic echo canceller backed by a linear FDAF filter.
///
/// Call [`process_render`](Aec::process_render) with each far-end frame first,
/// then [`process_capture`](Aec::process_capture) with the mic frame. Both
/// methods accept frames of any size — the internal ring-buffers decouple the
/// mixer's 10 ms tick from the FDAF's 42 ms processing block.
pub struct Aec {
    fdaf: FdafAec,
    /// Far-end (render) samples waiting to be processed.
    render_prod: <HeapRb<f32> as Split>::Prod,
    render_cons: <HeapRb<f32> as Split>::Cons,
    /// Near-end (mic) samples waiting to be processed.
    capture_prod: <HeapRb<f32> as Split>::Prod,
    capture_cons: <HeapRb<f32> as Split>::Cons,
    /// Echo-cancelled output samples ready for the mixer.
    output_prod: <HeapRb<f32> as Split>::Prod,
    output_cons: <HeapRb<f32> as Split>::Cons,
    /// Set after a panic in the FDAF; mic passes through uncancelled.
    disabled: bool,
    /// Pre-delay in samples applied once to the render buffer on first call.
    expected_delay_samples: usize,
    sample_rate: u32,
    /// Whether the pre-delay silence has already been pushed.
    initialized_delay: bool,
}

impl Aec {
    /// Creates a new `Aec` at the given sample rate without a delay hint.
    /// Real callers should use [`new_with_default_delay_hint`](Self::new_with_default_delay_hint)
    /// instead; `new` exists so tests can compare with and without the hint.
    pub fn new(sample_rate: u32) -> Self {
        // Buffers must hold several FDAF frames plus the full delay hint.
        // At 48 kHz, 50 ms = 2400 samples; 32768 gives plenty of headroom.
        let (render_prod, render_cons) = HeapRb::<f32>::new(32768).split();
        let (capture_prod, capture_cons) = HeapRb::<f32>::new(32768).split();
        let (output_prod, output_cons) = HeapRb::<f32>::new(32768).split();

        // step_size of 0.01 gives stable, convergent adaptation without
        // diverging on the 50 ms render/capture misalignment that a faster
        // rate (e.g. 0.5) would see before the delay hint takes effect.
        let fdaf = FdafAec::new(FFT_SIZE, 0.01);

        Self {
            fdaf,
            render_prod,
            render_cons,
            capture_prod,
            capture_cons,
            output_prod,
            output_cons,
            disabled: false,
            expected_delay_samples: 0,
            sample_rate,
            initialized_delay: false,
        }
    }

    /// Creates an `Aec` with [`EXPECTED_ACOUSTIC_DELAY_MS`] pre-applied.
    /// This is what every real caller should use.
    pub fn new_with_default_delay_hint(sample_rate: u32) -> Self {
        let mut aec = Self::new(sample_rate);
        aec.set_expected_delay_ms(EXPECTED_ACOUSTIC_DELAY_MS);
        aec
    }

    /// Sets the expected acoustic delay between render and capture in ms.
    /// This is used to pre-delay the render buffer so the filter sees aligned
    /// signals. Safe to call multiple times; only the last value is used.
    pub fn set_expected_delay_ms(&mut self, delay_ms: i32) {
        if delay_ms > 0 {
            self.expected_delay_samples = (delay_ms as u32 * self.sample_rate / 1000) as usize;
        }
    }

    /// Feeds a far-end (render) frame to the canceller.
    ///
    /// Must be called before the capture frame of the same tick.
    /// Silently drops frames once the internal buffer overflows (which should
    /// never happen under normal operation).
    pub fn process_render(&mut self, frame: &[f32]) {
        if self.disabled {
            return;
        }
        // On the very first render frame, pre-fill the render buffer with
        // `expected_delay_samples` of silence so that render and capture are
        // temporally aligned when the FDAF first processes them.
        if !self.initialized_delay {
            if self.expected_delay_samples > 0 {
                let zeros = vec![0.0f32; self.expected_delay_samples];
                let _ = self.render_prod.push_slice(&zeros);
            }
            self.initialized_delay = true;
        }
        let _ = self.render_prod.push_slice(frame);
    }

    /// Removes the echo of previously-rendered audio from a mic frame, in place.
    ///
    /// Processes as many FDAF blocks as both render and capture buffers allow,
    /// then writes the echo-cancelled result back into `frame`. If fewer
    /// processed samples are available than `frame.len()` (only during the
    /// initial fill period), outputs silence for those samples.
    pub fn process_capture(&mut self, frame: &mut [f32]) {
        if self.disabled {
            return;
        }

        let _ = self.capture_prod.push_slice(frame);

        while self.render_cons.occupied_len() >= FRAME_SIZE
            && self.capture_cons.occupied_len() >= FRAME_SIZE
        {
            let mut render_chunk = vec![0.0f32; FRAME_SIZE];
            let mut capture_chunk = vec![0.0f32; FRAME_SIZE];

            self.render_cons.pop_slice(&mut render_chunk);
            self.capture_cons.pop_slice(&mut capture_chunk);

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.fdaf.process(&render_chunk, &capture_chunk)
            }));

            match result {
                Ok(out_chunk) => {
                    let _ = self.output_prod.push_slice(&out_chunk);
                }
                Err(_) => {
                    self.disabled = true;
                    tracing::error!(
                        "FDAF AEC panicked; disabling echo cancellation for the session \
                         (mic will pass through uncancelled)"
                    );
                    break;
                }
            }
        }

        if self.output_cons.occupied_len() >= frame.len() {
            self.output_cons.pop_slice(frame);
        } else {
            // Output buffer has not yet accumulated a full frame (only during
            // the initial FRAME_SIZE fill period). Output silence rather than
            // passing raw mic, which would skip the render-side pre-delay and
            // corrupt the filter's alignment.
            frame.fill(0.0);
        }
    }

    /// Drains every sample still owed to the mic timeline.
    ///
    /// Order is oldest-first: already-processed output, then raw capture that
    /// had entered the delay line but not yet produced output. Render-side
    /// leftovers are discarded (no mic content). Called when `Aec` is replaced
    /// so the mixer can prepend these samples and avoid a speech gap.
    pub fn drain_pending(&mut self) -> Vec<f32> {
        let out_n = self.output_cons.occupied_len();
        let cap_n = self.capture_cons.occupied_len();
        let mut buf = vec![0.0f32; out_n + cap_n];
        if out_n > 0 {
            self.output_cons.pop_slice(&mut buf[..out_n]);
        }
        if cap_n > 0 {
            self.capture_cons.pop_slice(&mut buf[out_n..]);
        }
        // Drop render leftovers — they carry no near-end speech.
        let render_n = self.render_cons.occupied_len();
        if render_n > 0 {
            let mut discard = vec![0.0f32; render_n];
            self.render_cons.pop_slice(&mut discard);
        }
        buf
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
        let frame = rate as usize / 100; // 10 ms
        let mut aec = Aec::new(rate);

        // 3s of a 440 Hz tone as far-end; mic hears it scaled and delayed.
        let tone: Vec<f32> = (0..rate as usize * 3)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.5)
            .collect();
        let delay = rate as usize * 20 / 1000; // 20 ms simulated echo delay

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
            "echo should converge: early={leaked_energy_early:.4}, late={leaked_energy_late:.4}"
        );
    }

    /// `drain_pending` must hand back buffered output (and any unprocessed
    /// capture) so a replacement does not drop mic speech mid-session.
    #[test]
    fn drain_pending_transfers_samples_on_replacement() {
        let rate = 48_000u32;
        let frame_len = rate as usize / 100;
        let mut aec = Aec::new(rate);

        // Feed enough frames to fill the output buffer past one FDAF block.
        let far = vec![0.1f32; frame_len];
        let mut mic = vec![0.05f32; frame_len];
        for _ in 0..30 {
            aec.process_render(&far);
            aec.process_capture(&mut mic);
        }

        let drained = aec.drain_pending();
        assert!(
            !drained.is_empty(),
            "drain_pending should return pending samples"
        );
        assert_eq!(aec.output_cons.occupied_len(), 0);
        assert_eq!(aec.capture_cons.occupied_len(), 0);
        assert_eq!(aec.render_cons.occupied_len(), 0);
    }

    /// Once disabled (e.g. after a panic), process_render and process_capture
    /// must leave the mic frame completely untouched.
    #[test]
    fn process_calls_are_no_ops_once_disabled() {
        let rate = 48_000u32;
        let frame_len = rate as usize / 100;
        let mut aec = Aec::new(rate);
        aec.disabled = true;

        let far = vec![0.5f32; frame_len];
        let expected = vec![0.42f32; frame_len];
        let mut mic = expected.clone();

        aec.process_render(&far);
        aec.process_capture(&mut mic);

        assert_eq!(
            mic, expected,
            "a disabled Aec must leave the mic frame untouched"
        );
    }
}
