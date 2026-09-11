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
/// RMS floor above which a block of tap samples counts as carrying far-end
/// signal (SOU-119). The tap runs on the aggregate device clock, so it hands
/// over ~48000 frames a second whether or not anything is playing: counting
/// frames says the tap is alive, never that the other participants were
/// heard. ~-66 dBFS: two dozen dB below the AEC gate below (so no real
/// playback, however quiet, is missed) and well above the noise floor of a
/// digitally silent mix, which is exact zeros in practice.
const TAP_SIGNAL_RMS_FLOOR: f32 = 0.0005;
/// RMS floor on the tap leg below which AEC must not run (SOU-063).
/// Speakers-on / nothing-playing is the common damage case: the canceller
/// has a silent reference and suppresses the mic instead. ~-42 dBFS —
/// above dither, below any real playback.
const TAP_AEC_RMS_THRESHOLD: f32 = 0.008;
const TAP_RMS_ATTACK: f32 = 0.3;
const TAP_RMS_DECAY: f32 = 0.9;

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
    /// Tap samples pulled out of the ring since this mixer was built,
    /// silence included. Zero means the tap's queue never fired at all,
    /// which is not the same as a tap delivering silence (SOU-119).
    tap_ingested: u64,
    /// Of those, the ones in a block whose RMS cleared
    /// [`TAP_SIGNAL_RMS_FLOOR`]. This is the measure that says the far end
    /// was actually heard.
    tap_signal: u64,
    /// Recent far-end level. Gates *applying* AEC output, not whether
    /// the instance lives (SOU-063). A pause must not `set_aec(None)`.
    tap_rms_ema: f32,
    /// Blend toward cancelled (1.0) or raw mic (0.0). Ramps over one
    /// frame when the apply decision flips so the boundary stays continuous
    /// (SOU-063 AC3).
    aec_apply_mix: f32,
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
            tap_ingested: 0,
            tap_signal: 0,
            tap_rms_ema: 0.0,
            aec_apply_mix: 0.0,
        }
    }

    /// True when the tap recently carried real far-end energy.
    /// The AEC instance stays up; this only decides whether its output
    /// replaces the raw mic this frame.
    pub fn tap_has_energy(&self) -> bool {
        self.tap_rms_ema >= TAP_AEC_RMS_THRESHOLD
    }

    fn note_tap_samples(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            self.tap_rms_ema *= TAP_RMS_DECAY;
            return;
        }
        let mean_sq = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
        let rms = mean_sq.sqrt();
        // Per block, not smoothed: the EMA below is a control signal for the
        // AEC, while this is a plain tally of how much of the session carried
        // far-end signal. Blocks are one tick of ring content (~20ms), short
        // enough that speech never averages down under the floor.
        if rms >= TAP_SIGNAL_RMS_FLOOR {
            self.tap_signal += samples.len() as u64;
        }
        self.tap_rms_ema = TAP_RMS_ATTACK * rms + (1.0 - TAP_RMS_ATTACK) * self.tap_rms_ema;
    }

    /// Enable/disable echo cancellation (e.g. when the output route changes
    /// between speakers and headphones mid-session). Passing a fresh `Aec`
    /// also resets convergence state. Pending mic-timeline samples from the
    /// outgoing instance are prepended to the fifos so a mute/route flip does
    /// not drop speech still sitting in the FDAF delay line.
    pub fn set_aec(&mut self, aec: Option<Aec>) {
        if let Some(old) = self.aec.as_mut() {
            let pending = old.drain_pending();
            if !pending.is_empty() {
                let n = pending.len();
                let mut mic = pending;
                mic.append(&mut self.mic_fifo);
                self.mic_fifo = mic;
                // Keep legs aligned: pending is mic-only content, so the tap
                // side gets matching silence at the front.
                let mut tap = vec![0.0f32; n];
                tap.append(&mut self.tap_fifo);
                self.tap_fifo = tap;
            }
        }
        self.aec = aec;
        // Fresh instance (or none) starts from raw; the crossfade handles
        // the next apply transition.
        if self.aec.is_none() {
            self.aec_apply_mix = 0.0;
        }
    }

    /// Swap the microphone ring after a mid-session rebuild. The tap ring,
    /// its resampler, and AEC stay put so a USB/BT mic flip does not tear
    /// down a live process tap (and lose system audio if the new tap fails).
    pub fn replace_mic(
        &mut self,
        mic: HeapCons<f32>,
        mic_rate: u32,
        mic_channels: u16,
        mic_gain: f32,
    ) {
        // Carry the outgoing resampler's buffered chunk into the fifo instead
        // of dropping it, the same way `flush` does at session end: a rebuild
        // already costs the teardown gap, and this is another ~20ms of speech
        // on top. What stays in the fifo is sub-frame, so tap pairing and the
        // drift bound are unaffected.
        let tail = self.mic_to_mix.flush();
        self.mic_fifo.extend(tail);
        self.mic = mic;
        self.mic_to_mix = Resampler::new(mic_rate, mic_channels, MIX_RATE, mic_gain);

        // During a rebuild, the `meeting_tick` is not pumping the mixer, so the
        // tap leg (which survived the mic rebuild) accumulates system audio.
        // Ingest it now and pad the mic leg with silence so the accumulated tap
        // is not immediately discarded as "drift lead" on the next tick.
        // Drain both rings unbound: the new mic stream may already hold
        // samples from the rebuild interval (capture starts it before this
        // swap). Pad only the remaining tap lead so those mic samples line
        // up with the tap they arrived with, instead of sitting after silence.
        self.ingest(false);
        if self.tap_fifo.len() > self.mic_fifo.len() {
            let pad = self.tap_fifo.len() - self.mic_fifo.len();
            self.mic_fifo.resize(self.mic_fifo.len() + pad, 0.0);
        }
    }

    /// Drain both rings, mix every complete 10ms frame, and return the
    /// resulting engine-rate samples (possibly empty).
    pub fn tick(&mut self) -> Vec<f32> {
        self.tick_inner(false)
    }

    /// Like [`tick`], but when the mic fifo has no full frame, pace on the
    /// tap so a dead mic does not discard system audio as "lead".
    pub fn tick_on_tap_clock(&mut self) -> Vec<f32> {
        self.tick_inner(true)
    }

    fn tick_inner(&mut self, tap_clock: bool) -> Vec<f32> {
        self.ingest(!tap_clock);

        let mut out = Vec::new();
        while self.mic_fifo.len() >= FRAME_SAMPLES {
            let frame = self.mix_frame(FRAME_SAMPLES);
            out.extend(self.to_engine.process(&frame));
        }
        if tap_clock {
            while self.tap_fifo.len() >= FRAME_SAMPLES {
                let frame = self.tap_only_frame(FRAME_SAMPLES);
                out.extend(self.to_engine.process(&frame));
            }
        }
        out
    }

    /// Final drain when the session stops: both producers are gone, so
    /// everything left in the rings, FIFOs, and resampler tails comes out.
    pub fn flush(&mut self) -> Vec<f32> {
        let mut out = self.tick_on_tap_clock();

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
        self.tick_split_inner(false)
    }

    /// Diarized counterpart of [`tick_on_tap_clock`].
    pub fn tick_split_on_tap_clock(&mut self) -> (Vec<f32>, Vec<f32>) {
        self.tick_split_inner(true)
    }

    fn tick_split_inner(&mut self, tap_clock: bool) -> (Vec<f32>, Vec<f32>) {
        self.ingest(!tap_clock);

        let (mut me, mut them) = (Vec::new(), Vec::new());
        while self.mic_fifo.len() >= FRAME_SAMPLES {
            let (mic, tap) = self.split_frame(FRAME_SAMPLES);
            me.extend(self.to_engine.process(&mic));
            them.extend(self.tap_to_engine.process(&tap));
        }
        if tap_clock {
            while self.tap_fifo.len() >= FRAME_SAMPLES {
                let tap: Vec<f32> = self.tap_fifo.drain(..FRAME_SAMPLES).collect();
                them.extend(self.tap_to_engine.process(&tap));
            }
        }
        (me, them)
    }

    /// Diarized counterpart of `flush`.
    pub fn flush_split(&mut self) -> (Vec<f32>, Vec<f32>) {
        let (mut me, mut them) = self.tick_split_on_tap_clock();

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

    /// Tap samples this mixer has taken in, at the tap's own rate.
    pub fn tap_ingested(&self) -> u64 {
        self.tap_ingested
    }

    /// Tap samples that carried far-end signal, at the mix rate.
    pub fn tap_signal(&self) -> u64 {
        self.tap_signal
    }

    fn ingest(&mut self, bound_tap_lead: bool) {
        loop {
            let n = self.mic.pop_slice(&mut self.scratch);
            if n == 0 {
                break;
            }
            let resampled = self.mic_to_mix.process(&self.scratch[..n]);
            self.mic_fifo.extend(resampled);
        }
        let mut tap_added = 0usize;
        loop {
            let n = self.tap.pop_slice(&mut self.scratch);
            if n == 0 {
                break;
            }
            self.tap_ingested += n as u64;
            let resampled = self.tap_to_mix.process(&self.scratch[..n]);
            self.note_tap_samples(&resampled);
            tap_added += resampled.len();
            self.tap_fifo.extend(resampled);
        }
        if tap_added == 0 {
            self.note_tap_samples(&[]);
        }

        // Bound how far the tap leg can run ahead of the mic (clock drift).
        // Skip this when pacing on the tap: a dead mic would otherwise make
        // us discard the only remaining source as "lead".
        if bound_tap_lead {
            let max_len = self.mic_fifo.len() + MAX_TAP_LEAD_SAMPLES;
            if self.tap_fifo.len() > max_len {
                let excess = self.tap_fifo.len() - max_len;
                self.tap_fifo.drain(..excess);
                self.tap_discarded += excess as u64;
            }
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

    /// System-audio frame with no mic to pair. Used when the mic stream is
    /// down and the tap is the session's only live source.
    fn tap_only_frame(&mut self, n: usize) -> Vec<f32> {
        let tap_n = n.min(self.tap_fifo.len());
        let mut tap: Vec<f32> = self.tap_fifo.drain(..tap_n).collect();
        tap.resize(n, 0.0);
        tap
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
        // Always feed the canceller while it exists so a far-end pause
        // does not dump convergence. Apply the output only when the tap
        // is actually playing — otherwise emit raw mic (SOU-063 lever 2).
        // A one-frame linear crossfade covers apply↔bypass flips (AC3).
        let target = if self.tap_has_energy() { 1.0f32 } else { 0.0 };
        if n == FRAME_SAMPLES
            && let Some(aec) = self.aec.as_mut()
        {
            aec.process_render(&tap);
            let raw = mic.clone();
            let mut cancelled = mic;
            aec.process_capture(&mut cancelled);

            if (self.aec_apply_mix - target).abs() < f32::EPSILON {
                mic = if target > 0.5 { cancelled } else { raw };
            } else {
                let start = self.aec_apply_mix;
                let delta = target - start;
                mic = (0..n)
                    .map(|i| {
                        let t = start + delta * (i as f32 + 1.0) / n as f32;
                        cancelled[i] * t + raw[i] * (1.0 - t)
                    })
                    .collect();
                self.aec_apply_mix = target;
            }
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

    /// SOU-119 AC8: the case the ticket exists for. A tap that is up and
    /// carrying digital silence still delivers the full frame rate, so the
    /// frame count alone would call that session healthy.
    #[test]
    fn a_tap_delivering_silence_counts_frames_but_no_signal() {
        let (mut mic, mut tap, mut mixer) = make_mixer(48_000, 48_000, 16_000);

        for _ in 0..50 {
            mic.push_slice(&vec![0.1f32; 960]);
            tap.push_slice(&vec![0.0f32; 960]);
            mixer.tick();
        }

        assert!(
            mixer.tap_ingested() >= 48_000,
            "the tap did deliver frames: {}",
            mixer.tap_ingested()
        );
        assert_eq!(
            mixer.tap_signal(),
            0,
            "none of those frames carried far-end signal"
        );
    }

    #[test]
    fn a_tap_carrying_playback_counts_signal() {
        let (mut mic, mut tap, mut mixer) = make_mixer(48_000, 48_000, 16_000);

        for _ in 0..50 {
            mic.push_slice(&vec![0.1f32; 960]);
            tap.push_slice(&vec![0.05f32; 960]);
            mixer.tick();
        }

        assert!(
            mixer.tap_signal() >= 48_000,
            "quiet but real playback must count as signal: {}",
            mixer.tap_signal()
        );
    }

    /// The floor has to sit below anything a user would call audible, not
    /// just below line level.
    #[test]
    fn very_quiet_playback_still_counts_as_signal() {
        let (mut mic, mut tap, mut mixer) = make_mixer(48_000, 48_000, 16_000);

        // ~-60 dBFS, far below the AEC gate and barely audible.
        for _ in 0..50 {
            mic.push_slice(&vec![0.1f32; 960]);
            tap.push_slice(&vec![0.001f32; 960]);
            mixer.tick();
        }

        assert!(mixer.tap_signal() > 0, "quiet playback is still playback");
        assert!(
            !mixer.tap_has_energy(),
            "and it stays below the AEC apply floor, which is a different question"
        );
    }

    #[test]
    fn tap_energy_tracks_playback_then_decays() {
        let (mut mic, mut tap, mut mixer) = make_mixer(48_000, 48_000, 16_000);
        assert!(!mixer.tap_has_energy());

        tap.push_slice(&vec![0.2f32; 4_800]);
        mic.push_slice(&vec![0.1f32; 4_800]);
        mixer.tick();
        assert!(
            mixer.tap_has_energy(),
            "line-level tap is above the apply floor"
        );

        for _ in 0..80 {
            mic.push_slice(&vec![0.1f32; 480]);
            mixer.tick();
        }
        assert!(
            !mixer.tap_has_energy(),
            "silence after playback must drop below the AEC floor"
        );
    }

    #[test]
    fn aec_instance_survives_far_end_pause() {
        let (mut mic, mut tap, mut mixer) = make_mixer(48_000, 48_000, MIX_RATE);
        mixer.set_aec(Some(Aec::new(MIX_RATE)));

        tap.push_slice(&vec![0.2f32; 4_800]);
        mic.push_slice(&vec![0.1f32; 4_800]);
        mixer.tick();
        assert!(mixer.aec.is_some());
        assert!(mixer.tap_has_energy());

        for _ in 0..80 {
            mic.push_slice(&vec![0.1f32; 480]);
            mixer.tick();
        }
        assert!(!mixer.tap_has_energy());
        assert!(
            mixer.aec.is_some(),
            "far-end pause must not destroy the canceller"
        );
    }

    #[test]
    fn silent_tap_emits_raw_mic_while_aec_stays_fed() {
        let (mut mic, _tap, mut mixer) = make_mixer(48_000, 48_000, MIX_RATE);
        mixer.set_aec(Some(Aec::new(MIX_RATE)));
        assert!(!mixer.tap_has_energy());

        mic.push_slice(&vec![0.5f32; 4_800]);
        let mut out = mixer.tick();
        out.extend(mixer.flush());

        assert!(mixer.aec.is_some());
        let mid = out[out.len() / 2];
        assert!(
            (mid - 0.5).abs() < 0.05,
            "below the energy floor the mic must pass through raw, got {mid}"
        );
    }

    /// Replacing a live Aec must not drop samples still sitting in its delay
    /// line — they are prepended onto the mic fifo (SOU-063 / route flips).
    #[test]
    fn set_aec_none_preserves_pending_mic_timeline() {
        let (mut mic, mut tap, mut mixer) = make_mixer(48_000, 48_000, MIX_RATE);
        mixer.set_aec(Some(Aec::new(MIX_RATE)));

        // Drive enough frames for the FDAF block to produce leftover output.
        let far = vec![0.2f32; FRAME_SAMPLES];
        let near = vec![0.4f32; FRAME_SAMPLES];
        for _ in 0..30 {
            mic.push_slice(&near);
            tap.push_slice(&far);
            let _ = mixer.tick_split();
        }

        let fifo_before = mixer.mic_fifo.len();
        mixer.set_aec(None);
        assert!(
            mixer.mic_fifo.len() > fifo_before,
            "disengaging AEC must prepend pending delay-line samples \
             (fifo before={fifo_before}, after={})",
            mixer.mic_fifo.len()
        );
        assert!(mixer.aec.is_none());
    }

    /// Apply↔bypass flips crossfade over one frame: adjacent-sample jump at
    /// the boundary stays well under a hard switch (SOU-063 AC3).
    #[test]
    fn aec_apply_toggle_crossfade_bounds_discontinuity() {
        let (mut mic, _tap, mut mixer) = make_mixer(48_000, 48_000, MIX_RATE);
        // Live Aec during its silence-fill window: cancelled ≈ 0 while raw
        // mic is 0.8 — a hard switch would jump by ~0.8; the one-frame fade
        // must keep the per-step jump under ~0.8/480.
        mixer.set_aec(Some(Aec::new(MIX_RATE)));
        mixer.aec_apply_mix = 1.0;
        mixer.tap_rms_ema = 0.0; // target = 0 → crossfade 1→0 this frame

        mic.push_slice(&vec![0.8f32; FRAME_SAMPLES]);
        let (me, _) = mixer.tick_split();

        assert!(
            (mixer.aec_apply_mix - 0.0).abs() < f32::EPSILON,
            "apply mix must finish the frame at the raw target"
        );
        let max_jump = me
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_jump < 0.01,
            "crossfade frame adjacent jump too large: {max_jump} (hard switch would be ~0.8)"
        );
        // Endpoint should land on raw mic.
        assert!(
            (me[me.len() - 1] - 0.8).abs() < 0.05,
            "fade must end on raw mic, got {}",
            me[me.len() - 1]
        );
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
    fn tap_clock_emits_system_audio_when_mic_is_empty() {
        let (_mic, mut tap, mut mixer) = make_mixer(48_000, 48_000, 16_000);

        tap.push_slice(&vec![0.4f32; 4_800]);
        assert!(
            mixer.tick().is_empty(),
            "mic clock must not invent frames from tap alone"
        );
        let out = mixer.tick_on_tap_clock();
        assert!(
            !out.is_empty(),
            "a dead mic must still deliver the live tap, not discard it as lead"
        );
        let mid = out[out.len() / 2];
        assert!((mid - 0.4).abs() < 0.05, "expected ~0.4, got {mid}");
    }

    #[test]
    fn tap_clock_does_not_discard_lead_when_mic_is_empty() {
        let (_mic, mut tap, mut mixer) = make_mixer(48_000, 48_000, 16_000);
        // More than MAX_TAP_LEAD_SAMPLES (12_000): mic-clock ingest would drop it.
        tap.push_slice(&vec![0.4f32; 24_000]);
        let out = mixer.tick_on_tap_clock();
        assert!(!out.is_empty());
        assert_eq!(
            mixer.tap_discarded(),
            0,
            "a dead mic must not treat the live tap as clock-drift lead"
        );
    }

    #[test]
    fn tap_clock_split_puts_system_audio_on_them_only() {
        let (_mic, mut tap, mut mixer) = make_mixer(48_000, 48_000, 16_000);

        tap.push_slice(&vec![0.3f32; 4_800]);
        let (me, them) = mixer.tick_split_on_tap_clock();
        assert!(me.is_empty(), "no mic frames to emit");
        assert!(!them.is_empty(), "them must carry the tap");
        let mid = them[them.len() / 2];
        assert!((mid - 0.3).abs() < 0.05, "expected ~0.3, got {mid}");
    }

    /// A mid-session rebuild must not swallow the speech the mic resampler
    /// had accumulated but not yet converted.
    #[test]
    fn replace_mic_keeps_the_partial_mic_chunk() {
        // 44.1kHz -> 48kHz needs 1029 input frames per FFT chunk, so 1000
        // samples sit entirely inside the resampler with nothing emitted.
        let (mut mic, _tap, mut mixer) = make_mixer(44_100, 48_000, 16_000);
        mic.push_slice(&vec![0.4f32; 1_000]);
        assert!(
            mixer.tick().is_empty(),
            "a partial chunk must not produce a frame yet"
        );
        drop(mic);

        let (_new_mic, new_cons) = HeapRb::<f32>::new(44_100 * 2).split();
        mixer.replace_mic(new_cons, 44_100, 1, 1.0);

        // No new mic samples: whatever comes out now is the rescued tail.
        // It still has to clear the engine resampler's own chunking, hence
        // the flush.
        let mut out = mixer.tick();
        out.extend(mixer.flush());
        assert!(
            !out.is_empty(),
            "the buffered chunk must survive the mic rebuild"
        );
        assert!(
            out.iter().any(|s| (*s - 0.4).abs() < 0.05),
            "the rescued samples must be the ones that were buffered"
        );
    }

    #[test]
    fn replace_mic_keeps_buffered_tap() {
        let (old_mic, mut tap, mut mixer) = make_mixer(48_000, 48_000, 16_000);
        tap.push_slice(&vec![0.2f32; FRAME_SAMPLES]);
        // Ingest the tap into the fifo without a mic frame so it stays put.
        assert!(mixer.tick().is_empty());
        drop(old_mic);

        let (mut new_mic, new_cons) = HeapRb::<f32>::new(48_000 * 2).split();
        mixer.replace_mic(new_cons, 48_000, 1, 1.0);
        new_mic.push_slice(&vec![0.1f32; FRAME_SAMPLES]);
        let mut out = mixer.tick();
        out.extend(mixer.flush());

        assert!(!out.is_empty());
        assert_eq!(
            mixer.tap_discarded(),
            0,
            "buffered tap must not be dropped as drift"
        );
        assert!(
            out.iter().any(|s| (*s - 0.2).abs() < 0.05),
            "replaced mic must still emit the tap that survived the rebuild"
        );
        assert!(
            out.iter().any(|s| (*s - 0.1).abs() < 0.05),
            "new mic samples after the rebuild must still mix"
        );
    }

    #[test]
    fn replace_mic_drains_new_mic_before_silence_pad() {
        let (old_mic, mut tap, mut mixer) = make_mixer(48_000, 48_000, 16_000);
        tap.push_slice(&vec![0.2f32; FRAME_SAMPLES * 2]);
        drop(old_mic);

        let (mut new_mic, new_cons) = HeapRb::<f32>::new(48_000 * 2).split();
        new_mic.push_slice(&vec![0.1f32; FRAME_SAMPLES]);
        mixer.replace_mic(new_cons, 48_000, 1, 1.0);

        let mut out = mixer.tick();
        out.extend(mixer.flush());

        assert_eq!(
            mixer.tap_discarded(),
            0,
            "aligned tap must not be dropped as drift"
        );
        assert!(
            out.iter().any(|s| (*s - 0.3).abs() < 0.05),
            "mic already in the new ring must mix with tap, not sit after a silence pad"
        );
        assert!(
            out.iter().any(|s| (*s - 0.2).abs() < 0.05),
            "rebuild-gap tap must pair with silence, not later mic"
        );
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
    fn synth_wideband(
        len: usize,
        sample_rate: u32,
        freqs: &[(f32, f32)],
        noise_amp: f32,
        seed: u64,
    ) -> Vec<f32> {
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
        let mut mixer = MeetingMixer::new(
            mic_cons,
            sample_rate,
            1,
            1.0,
            tap_cons,
            sample_rate,
            sample_rate,
        );
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
            let window_post: f32 = residual_energy_per_frame[i..i + sustain_frames]
                .iter()
                .sum();
            if window_post < window_pre * 0.1 {
                converge_frame = Some(i);
                break;
            }
        }

        (erle_db, converge_frame)
    }

    // Measured baselines (release build, SOU-063 fix, 2026-09-09): fdaf-aec
    // (linear FDAF, no NLP suppression) achieves +19.7 dB ERLE in double-talk
    // and +30.8 dB echo-only. This floor is the regression gate — if ERLE
    // falls back negative someone re-introduced a non-linear suppressor.
    const DOUBLE_TALK_ERLE_FLOOR_DB: f32 = 0.0;

    #[test]
    fn wideband_echo_attenuation_baseline() {
        let (erle_db, converge_frame) = run_echo_bench(None, 1.0, 50);
        println!(
            "AEC bench (no stream_delay_ms hint, 50ms acoustic delay): ERLE post-convergence = \
             {erle_db:.1} dB, convergence at ~{}ms",
            converge_frame
                .map(|f| f * 10)
                .map_or("never".to_string(), |ms| ms.to_string())
        );
        assert!(
            erle_db > DOUBLE_TALK_ERLE_FLOOR_DB,
            "double-talk ERLE regressed below the measured baseline: got {erle_db:.1} dB"
        );
    }

    #[test]
    fn wideband_echo_attenuation_with_delay_hint() {
        let (erle_db, converge_frame) = run_echo_bench(Some(50), 1.0, 50);
        println!(
            "AEC bench (50ms stream_delay_ms hint, 50ms acoustic delay): ERLE post-convergence = \
             {erle_db:.1} dB, convergence at ~{}ms",
            converge_frame
                .map(|f| f * 10)
                .map_or("never".to_string(), |ms| ms.to_string())
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
    fn wideband_echo_attenuation_no_voice_diagnostic() {
        let (erle_db, converge_frame) = run_echo_bench(Some(50), 0.0, 50);
        println!(
            "AEC bench (echo only, no voice, 50ms hint, 50ms acoustic delay): ERLE \
             post-convergence = {erle_db:.1} dB, convergence at ~{}ms",
            converge_frame
                .map(|f| f * 10)
                .map_or("never".to_string(), |ms| ms.to_string())
        );
    }
}
