//! Two-source mixer for meeting mode: microphone + system audio.
//!
//! Real-time callbacks (cpal, tap IOProc) only push raw samples into SPSC
//! ring buffers; this mixer runs on the audio-capture thread's tick and does
//! everything else: resample both sources to a common 48kHz, pair them into
//! 10ms frames (the unit echo cancellation works on), sum, and resample to
//! the engine rate. The microphone is the pacing clock — when the system is
//! silent or the tap is missing, its leg is just zeros, so mic-only is the
//! natural degenerate case rather than an error path.

use ringbuf::HeapCons;
use ringbuf::traits::Consumer;

use super::aec::Aec;
use super::resampler::Resampler;

/// Common processing rate; 48kHz matches the tap's native rate and is a
/// supported AEC band split rate.
pub const MIX_RATE: u32 = 48_000;
/// 10ms at the mix rate.
pub const FRAME_SAMPLES: usize = 480;
/// How far the system-audio leg may run ahead of the mic before old samples
/// are discarded. Bounds clock drift between the two devices; transcription
/// tolerates the resulting sub-frame discontinuity.
const MAX_TAP_LEAD_SAMPLES: usize = MIX_RATE as usize / 4;

pub struct MeetingMixer {
    mic: HeapCons<f32>,
    tap: HeapCons<f32>,
    mic_to_mix: Resampler,
    tap_to_mix: Resampler,
    to_engine: Resampler,
    /// Second engine-rate resampler used only in diarized (split) mode, so the
    /// system-audio leg has its own resampler state independent of the mic leg.
    tap_to_engine: Resampler,
    mic_fifo: Vec<f32>,
    tap_fifo: Vec<f32>,
    scratch: Vec<f32>,
    /// Echo cancellation, active only when the output routes to the
    /// built-in speakers (headphones can't leak into the mic).
    aec: Option<Aec>,
    /// Tap samples discarded to bound drift; logged at session end.
    tap_discarded: u64,
}

impl MeetingMixer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mic: HeapCons<f32>,
        mic_rate: u32,
        mic_channels: u16,
        mic_gain: f32,
        tap: HeapCons<f32>,
        tap_rate: u32,
        engine_rate: u32,
    ) -> Self {
        Self {
            mic,
            tap,
            // The mic gain is engine-specific amplification for voice input;
            // the system-audio leg is already line-level, so it stays at 1.0.
            mic_to_mix: Resampler::new(mic_rate, mic_channels, MIX_RATE, mic_gain),
            tap_to_mix: Resampler::new(tap_rate, 1, MIX_RATE, 1.0),
            to_engine: Resampler::new(MIX_RATE, 1, engine_rate, 1.0),
            tap_to_engine: Resampler::new(MIX_RATE, 1, engine_rate, 1.0),
            mic_fifo: Vec::new(),
            tap_fifo: Vec::new(),
            scratch: vec![0.0; 4096],
            aec: None,
            tap_discarded: 0,
        }
    }

    /// Enable/disable echo cancellation (e.g. when the output route changes
    /// between speakers and headphones mid-session). Passing a fresh `Aec`
    /// also resets convergence state.
    pub fn set_aec(&mut self, aec: Option<Aec>) {
        self.aec = aec;
    }

    /// Drain both rings, mix every complete 10ms frame, and return the
    /// resulting engine-rate samples (possibly empty).
    pub fn tick(&mut self) -> Vec<f32> {
        self.ingest();

        let mut out = Vec::new();
        while self.mic_fifo.len() >= FRAME_SAMPLES {
            let frame = self.mix_frame(FRAME_SAMPLES);
            out.extend(self.to_engine.process(&frame));
        }
        out
    }

    /// Final drain when the session stops: both producers are gone, so
    /// everything left in the rings, FIFOs, and resampler tails comes out.
    pub fn flush(&mut self) -> Vec<f32> {
        let mut out = self.tick();

        let mic_tail = self.mic_to_mix.flush();
        self.mic_fifo.extend(mic_tail);
        let tap_tail = self.tap_to_mix.flush();
        self.tap_fifo.extend(tap_tail);

        while !self.mic_fifo.is_empty() {
            let n = self.mic_fifo.len().min(FRAME_SAMPLES);
            let frame = self.mix_frame(n);
            out.extend(self.to_engine.process(&frame));
        }
        // Without a mic the remaining system audio still matters (e.g. the
        // last words of a remote participant).
        while !self.tap_fifo.is_empty() {
            let n = self.tap_fifo.len().min(FRAME_SAMPLES);
            let frame: Vec<f32> = self.tap_fifo.drain(..n).collect();
            out.extend(self.to_engine.process(&frame));
        }
        out.extend(self.to_engine.flush());
        out
    }

    /// Diarized counterpart of `tick`: instead of summing the two legs, return
    /// them separately at the engine rate — `(me, them)` = (echo-cancelled mic,
    /// system audio). Each leg has its own engine resampler so their stream
    /// states don't interfere.
    pub fn tick_split(&mut self) -> (Vec<f32>, Vec<f32>) {
        self.ingest();

        let (mut me, mut them) = (Vec::new(), Vec::new());
        while self.mic_fifo.len() >= FRAME_SAMPLES {
            let (mic, tap) = self.split_frame(FRAME_SAMPLES);
            me.extend(self.to_engine.process(&mic));
            them.extend(self.tap_to_engine.process(&tap));
        }
        (me, them)
    }

    /// Diarized counterpart of `flush`.
    pub fn flush_split(&mut self) -> (Vec<f32>, Vec<f32>) {
        let (mut me, mut them) = self.tick_split();

        let mic_tail = self.mic_to_mix.flush();
        self.mic_fifo.extend(mic_tail);
        let tap_tail = self.tap_to_mix.flush();
        self.tap_fifo.extend(tap_tail);

        // Mic leg is the pacing clock; pair with tap for AEC where possible.
        while !self.mic_fifo.is_empty() {
            let n = self.mic_fifo.len().min(FRAME_SAMPLES);
            let (mic, tap) = self.split_frame(n);
            me.extend(self.to_engine.process(&mic));
            them.extend(self.tap_to_engine.process(&tap));
        }
        // Remaining system audio with no mic to pair → straight to "them".
        while !self.tap_fifo.is_empty() {
            let n = self.tap_fifo.len().min(FRAME_SAMPLES);
            let tap: Vec<f32> = self.tap_fifo.drain(..n).collect();
            them.extend(self.tap_to_engine.process(&tap));
        }
        me.extend(self.to_engine.flush());
        them.extend(self.tap_to_engine.flush());
        (me, them)
    }

    pub fn tap_discarded(&self) -> u64 {
        self.tap_discarded
    }

    fn ingest(&mut self) {
        loop {
            let n = self.mic.pop_slice(&mut self.scratch);
            if n == 0 {
                break;
            }
            let resampled = self.mic_to_mix.process(&self.scratch[..n]);
            self.mic_fifo.extend(resampled);
        }
        loop {
            let n = self.tap.pop_slice(&mut self.scratch);
            if n == 0 {
                break;
            }
            let resampled = self.tap_to_mix.process(&self.scratch[..n]);
            self.tap_fifo.extend(resampled);
        }

        // Bound how far the tap leg can run ahead of the mic (clock drift,
        // or a stalled mic device): drop the oldest excess.
        let max_len = self.mic_fifo.len() + MAX_TAP_LEAD_SAMPLES;
        if self.tap_fifo.len() > max_len {
            let excess = self.tap_fifo.len() - max_len;
            self.tap_fifo.drain(..excess);
            self.tap_discarded += excess as u64;
        }
    }

    /// Take `n` mic samples, cancel the echo of the system audio in them
    /// (speakers only), then sum the tap leg over them (zeros when the
    /// system is silent) and clamp.
    fn mix_frame(&mut self, n: usize) -> Vec<f32> {
        let (mut mic, tap) = self.split_frame(n);
        for (sample, tap) in mic.iter_mut().zip(&tap) {
            *sample = (*sample + *tap).clamp(-1.0, 1.0);
        }
        mic
    }

    /// Drain one frame from each leg and echo-cancel the mic, but return the
    /// two legs separately (mic, tap) rather than summed. `tap` is zero-padded
    /// to `n` so the two are always the same length.
    fn split_frame(&mut self, n: usize) -> (Vec<f32>, Vec<f32>) {
        let mut mic: Vec<f32> = self.mic_fifo.drain(..n).collect();
        let tap_n = n.min(self.tap_fifo.len());
        let mut tap: Vec<f32> = self.tap_fifo.drain(..tap_n).collect();
        tap.resize(n, 0.0);

        // AEC works on exact 10ms frames; the only shorter frames are the
        // final flush tail, where skipping cancellation is harmless.
        if n == FRAME_SAMPLES
            && let Some(aec) = self.aec.as_mut()
        {
            aec.process_render(&tap);
            aec.process_capture(&mut mic);
        }

        (mic, tap)
    }
}

#[cfg(test)]
mod tests {
    use ringbuf::HeapRb;
    use ringbuf::traits::{Producer, Split};

    use super::*;

    fn make_mixer(
        mic_rate: u32,
        tap_rate: u32,
        engine_rate: u32,
    ) -> (ringbuf::HeapProd<f32>, ringbuf::HeapProd<f32>, MeetingMixer) {
        let (mic_prod, mic_cons) = HeapRb::<f32>::new(mic_rate as usize * 2).split();
        let (tap_prod, tap_cons) = HeapRb::<f32>::new(tap_rate as usize * 2).split();
        let mixer = MeetingMixer::new(mic_cons, mic_rate, 1, 1.0, tap_cons, tap_rate, engine_rate);
        (mic_prod, tap_prod, mixer)
    }

    #[test]
    fn mixes_both_sources_sample_count() {
        let (mut mic, mut tap, mut mixer) = make_mixer(48_000, 48_000, 16_000);

        // 1s of mic, 1s of tap at matching rates.
        mic.push_slice(&vec![0.1f32; 48_000]);
        tap.push_slice(&vec![0.2f32; 48_000]);
        let mut out = mixer.tick();
        out.extend(mixer.flush());

        // 1s at 16kHz, within resampler latency tolerance.
        assert!(
            (out.len() as i64 - 16_000).unsigned_abs() < 2_000,
            "expected ~16000 samples, got {}",
            out.len()
        );
        // Steady-state samples carry both sources (0.1 + 0.2).
        let mid = out[out.len() / 2];
        assert!((mid - 0.3).abs() < 0.05, "expected ~0.3, got {mid}");
    }

    #[test]
    fn mic_only_when_tap_silent() {
        let (mut mic, _tap, mut mixer) = make_mixer(48_000, 48_000, 16_000);

        mic.push_slice(&vec![0.5f32; 24_000]);
        let mut out = mixer.tick();
        out.extend(mixer.flush());

        assert!(!out.is_empty());
        let mid = out[out.len() / 2];
        assert!((mid - 0.5).abs() < 0.05, "expected ~0.5, got {mid}");
    }

    #[test]
    fn tap_tail_flushed_without_mic() {
        let (_mic, mut tap, mut mixer) = make_mixer(48_000, 48_000, 16_000);

        tap.push_slice(&vec![0.4f32; 4_800]);
        assert!(mixer.tick().is_empty(), "no mic frames → no paced output");
        let out = mixer.flush();
        assert!(!out.is_empty(), "flush must drain the tap leg");
    }

    #[test]
    fn tap_lead_is_bounded() {
        let (mut mic, mut tap, mut mixer) = make_mixer(48_000, 48_000, 16_000);

        // Tap runs far ahead of the mic.
        tap.push_slice(&vec![0.2f32; 48_000]);
        mic.push_slice(&vec![0.1f32; 480]);
        mixer.tick();

        assert!(
            mixer.tap_discarded() > 0,
            "excess tap lead should be dropped"
        );
        assert!(mixer.tap_fifo.len() <= MAX_TAP_LEAD_SAMPLES + FRAME_SAMPLES);
    }

    #[test]
    fn split_keeps_sources_separate() {
        let (mut mic, mut tap, mut mixer) = make_mixer(48_000, 48_000, 16_000);

        mic.push_slice(&vec![0.1f32; 48_000]);
        tap.push_slice(&vec![0.2f32; 48_000]);
        let (mut me, mut them) = mixer.tick_split();
        let (me_tail, them_tail) = mixer.flush_split();
        me.extend(me_tail);
        them.extend(them_tail);

        // Each leg is ~1s at 16kHz and carries only its own source (not summed).
        assert!((me.len() as i64 - 16_000).unsigned_abs() < 2_000);
        assert!((them.len() as i64 - 16_000).unsigned_abs() < 2_000);
        let me_mid = me[me.len() / 2];
        let them_mid = them[them.len() / 2];
        assert!(
            (me_mid - 0.1).abs() < 0.05,
            "me should be ~0.1, got {me_mid}"
        );
        assert!(
            (them_mid - 0.2).abs() < 0.05,
            "them should be ~0.2, got {them_mid}"
        );
    }

    #[test]
    fn resamples_mismatched_rates() {
        // Mic at 44.1k stereo, tap at 48k, engine at 24k.
        let (mic_prod, mic_cons) = HeapRb::<f32>::new(44_100 * 4).split();
        let (tap_prod, tap_cons) = HeapRb::<f32>::new(48_000 * 2).split();
        let mut mic = mic_prod;
        let mut tap = tap_prod;
        let mut mixer = MeetingMixer::new(mic_cons, 44_100, 2, 1.0, tap_cons, 48_000, 24_000);

        mic.push_slice(&vec![0.1f32; 44_100 * 2]); // 1s stereo
        tap.push_slice(&vec![0.2f32; 48_000]); // 1s mono
        let mut out = mixer.tick();
        out.extend(mixer.flush());

        assert!(
            (out.len() as i64 - 24_000).unsigned_abs() < 3_000,
            "expected ~24000 samples, got {}",
            out.len()
        );
    }
}

/// Echo-cancellation efficiency bench, run through the exact mixer
/// integration (10ms frames at `MIX_RATE`, `split_frame`'s render-then-capture
/// ordering) rather than the `Aec` wrapper in isolation. Ignored by default:
/// several seconds of synthetic audio at 48kHz is too slow for the normal
/// test loop. Run explicitly with:
///   cargo test --release --manifest-path src-tauri/Cargo.toml audio::mixer::aec_bench -- --ignored --nocapture
#[cfg(test)]
mod aec_bench {
    use ringbuf::HeapRb;
    use ringbuf::traits::{Producer, Split};

    use super::*;
    use crate::audio::aec::Aec;

    /// Deterministic xorshift64 PRNG so the bench is reproducible without an
    /// external `rand` dependency.
    struct Xorshift(u64);

    impl Xorshift {
        fn next_unit(&mut self) -> f32 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            // Top 24 bits as a value in roughly [-1, 1).
            ((x >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
        }
    }

    /// Musical-ish frequencies with decreasing amplitude, meant to stand in
    /// for video/music playback content (broadband, not a pure tone).
    const VIDEO_FREQS: &[(f32, f32)] = &[
        (220.0, 0.22),
        (523.0, 0.18),
        (1046.0, 0.14),
        (1760.0, 0.10),
        (2637.0, 0.07),
    ];

    /// A voice fundamental plus harmonics, distinct from `VIDEO_FREQS`, to
    /// stand in for the user's own speech (near-end, double-talk). Kept well
    /// under full scale together with the echo so `capture`'s clamp to
    /// [-1, 1] is a safety net, not a routine clipper: real gain-staged mic
    /// input doesn't clip, and clipping would distort capture in a way no
    /// linear AEC can undo, swamping the echo-cancellation measurement with
    /// an unrelated clipping artifact.
    const VOICE_FREQS: &[(f32, f32)] = &[
        (120.0, 0.15),
        (240.0, 0.10),
        (360.0, 0.06),
        (480.0, 0.04),
        (720.0, 0.025),
    ];

    /// Sum of sinusoids at `freqs` plus a touch of low-pass-filtered noise,
    /// so the signal has broadband content like real speech/video audio
    /// rather than a single tone.
    fn synth_wideband(len: usize, sample_rate: u32, freqs: &[(f32, f32)], noise_amp: f32, seed: u64) -> Vec<f32> {
        let mut rng = Xorshift(seed);
        let mut noise_state = 0.0f32;
        (0..len)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                let tonal: f32 = freqs
                    .iter()
                    .map(|(freq, amp)| amp * (t * freq * std::f32::consts::TAU).sin())
                    .sum();
                // One-pole low-pass on white noise: broadens the spectrum
                // without the harshness of raw white noise.
                noise_state = 0.9 * noise_state + 0.1 * rng.next_unit();
                (tonal + noise_amp * noise_state).clamp(-1.0, 1.0)
            })
            .collect()
    }

    /// Feeds a synthetic "video playing on the speakers while the user
    /// talks" scenario through `MeetingMixer` and reports (does not just
    /// assert) how effectively the AEC integration attenuates the echo:
    /// approximate ERLE post-convergence and roughly when convergence
    /// happens. `expected_delay_hint_ms` mirrors what `Aec` would pass to
    /// `set_stream_delay_ms` when that hint is wired up; pass `None` to
    /// measure the current (no hint) baseline.
    fn run_echo_bench(
        expected_delay_hint_ms: Option<i32>,
        voice_scale: f32,
        delay_ms: u32,
    ) -> (f32, Option<usize>) {
        let sample_rate = MIX_RATE;
        let delay_samples = (sample_rate as usize * delay_ms as usize) / 1000;
        let attenuation = 0.3f32;
        let duration_s = 8.0f32;
        let total_samples = (sample_rate as f32 * duration_s) as usize;

        let render = synth_wideband(total_samples, sample_rate, VIDEO_FREQS, 0.05, 0xC0FFEE);
        let voice: Vec<f32> = synth_wideband(total_samples, sample_rate, VOICE_FREQS, 0.02, 0xBEEF)
            .into_iter()
            .map(|s| s * voice_scale)
            .collect();

        // Ground truth: exactly what the acoustic path adds to the mic and
        // exactly what the user said, so residual echo can be estimated
        // after the fact as `output - voice` (see comment below).
        let echo: Vec<f32> = (0..total_samples)
            .map(|n| {
                if n >= delay_samples {
                    render[n - delay_samples] * attenuation
                } else {
                    0.0
                }
            })
            .collect();
        let capture: Vec<f32> = (0..total_samples)
            .map(|n| (voice[n] + echo[n]).clamp(-1.0, 1.0))
            .collect();

        let (mut mic_prod, mic_cons) = HeapRb::<f32>::new(sample_rate as usize).split();
        let (mut tap_prod, tap_cons) = HeapRb::<f32>::new(sample_rate as usize).split();
        // engine_rate == MIX_RATE: no cross-resampling, so the bench measures
        // the AEC's contribution in isolation from resampler artifacts.
        let mut mixer = MeetingMixer::new(mic_cons, sample_rate, 1, 1.0, tap_cons, sample_rate, sample_rate);
        let mut aec = Aec::new(sample_rate);
        if let Some(hint) = expected_delay_hint_ms {
            aec.set_expected_delay_ms(hint);
        }
        mixer.set_aec(Some(aec));

        let n_frames = total_samples / FRAME_SAMPLES;
        let mut echo_energy_per_frame = Vec::with_capacity(n_frames);
        let mut residual_energy_per_frame = Vec::with_capacity(n_frames);

        for f in 0..n_frames {
            let start = f * FRAME_SAMPLES;
            let end = start + FRAME_SAMPLES;
            mic_prod.push_slice(&capture[start..end]);
            tap_prod.push_slice(&render[start..end]);

            let (me, _them) = mixer.tick_split();
            assert_eq!(
                me.len(),
                FRAME_SAMPLES,
                "matching rates should pass one 10ms frame through per tick"
            );

            let echo_energy: f32 = echo[start..end].iter().map(|s| s * s).sum();
            // Residual echo estimate: the AEC doesn't know `voice` separately,
            // but we generated it, so subtracting it from the output isolates
            // what the AEC left behind (residual echo plus any voice
            // distortion the AEC itself introduced).
            let residual_energy: f32 = me
                .iter()
                .zip(&voice[start..end])
                .map(|(o, v)| (o - v).powi(2))
                .sum();

            echo_energy_per_frame.push(echo_energy);
            residual_energy_per_frame.push(residual_energy);
        }

        // Post-convergence window: last quarter of the run.
        let tail = n_frames / 4;
        let pre: f32 = echo_energy_per_frame[n_frames - tail..].iter().sum();
        let post: f32 = residual_energy_per_frame[n_frames - tail..].iter().sum();
        let erle_db = 10.0 * (pre / post.max(1e-9)).log10();

        // Convergence point: first frame after which a 500ms sliding window
        // of residual energy stays below 10% (-10dB) of the echo energy in
        // that same window.
        let sustain_frames = 50;
        let mut converge_frame = None;
        for i in 0..n_frames.saturating_sub(sustain_frames) {
            let window_pre: f32 = echo_energy_per_frame[i..i + sustain_frames].iter().sum();
            let window_post: f32 = residual_energy_per_frame[i..i + sustain_frames].iter().sum();
            if window_post < window_pre * 0.1 {
                converge_frame = Some(i);
                break;
            }
        }

        (erle_db, converge_frame)
    }

    // Measured baselines (release build, this bench, 2026-07-18): double-talk
    // (voice + echo together) comes out around -10dB, i.e. sonora's NLP stage
    // leaves the output *further* from clean voice than the raw uncancelled
    // echo would have been, both with and without the `stream_delay_ms` hint.
    // Echo-only (no voice, see the diagnostic below) attenuates by a modest
    // +3.6dB. That's weak for AEC3 and corroborates the production incident's
    // report of echo passing through even before the crate panicked; it is
    // not something this fix attempts to solve (see the investigation
    // writeup). These asserts are regression floors against the measured
    // baseline, not aspirational targets — tighten them if cancellation
    // quality is improved later.
    const DOUBLE_TALK_ERLE_FLOOR_DB: f32 = -14.0;

    #[test]
    #[ignore = "multi-second synthetic bench, run explicitly with -- --ignored --nocapture"]
    fn wideband_echo_attenuation_baseline() {
        let (erle_db, converge_frame) = run_echo_bench(None, 1.0, 50);
        println!(
            "AEC bench (no stream_delay_ms hint, 50ms acoustic delay): ERLE post-convergence = \
             {erle_db:.1} dB, convergence at ~{}ms",
            converge_frame.map(|f| f * 10).map_or("never".to_string(), |ms| ms.to_string())
        );
        assert!(
            erle_db > DOUBLE_TALK_ERLE_FLOOR_DB,
            "double-talk ERLE regressed below the measured baseline: got {erle_db:.1} dB"
        );
    }

    #[test]
    #[ignore = "multi-second synthetic bench, run explicitly with -- --ignored --nocapture"]
    fn wideband_echo_attenuation_with_delay_hint() {
        let (erle_db, converge_frame) = run_echo_bench(Some(50), 1.0, 50);
        println!(
            "AEC bench (50ms stream_delay_ms hint, 50ms acoustic delay): ERLE post-convergence = \
             {erle_db:.1} dB, convergence at ~{}ms",
            converge_frame.map(|f| f * 10).map_or("never".to_string(), |ms| ms.to_string())
        );
        assert!(
            erle_db > DOUBLE_TALK_ERLE_FLOOR_DB,
            "double-talk ERLE regressed below the measured baseline: got {erle_db:.1} dB"
        );
    }

    /// Diagnostic (not a hard requirement): isolates echo-only cancellation
    /// (no near-end voice) through the exact same mixer integration, to tell
    /// whether a poor double-talk result comes from echo cancellation itself
    /// or from the double-talk/voice interaction. Measured baseline: +3.6dB.
    #[test]
    #[ignore = "multi-second synthetic bench, run explicitly with -- --ignored --nocapture"]
    fn wideband_echo_attenuation_no_voice_diagnostic() {
        let (erle_db, converge_frame) = run_echo_bench(Some(50), 0.0, 50);
        println!(
            "AEC bench (echo only, no voice, 50ms hint, 50ms acoustic delay): ERLE \
             post-convergence = {erle_db:.1} dB, convergence at ~{}ms",
            converge_frame.map(|f| f * 10).map_or("never".to_string(), |ms| ms.to_string())
        );
    }

}
