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
/// RMS floor on the tap leg below which AEC must not run (SOU-063).
/// Speakers-on / nothing-playing is the common damage case: the canceller
/// has a silent reference and suppresses the mic instead. ~-42 dBFS —
/// above dither, below any real playback.
const TAP_AEC_RMS_THRESHOLD: f32 = 0.008;
const TAP_RMS_ATTACK: f32 = 0.3;
const TAP_RMS_DECAY: f32 = 0.9;
/// Per-sample increment of the raw/cancelled blend: a full switch takes
/// exactly one 10ms frame (SOU-063).
const AEC_FADE_STEP: f32 = 1.0 / FRAME_SAMPLES as f32;

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
    /// Recent far-end level. Gates *applying* AEC output, not whether
    /// the instance lives (SOU-063). A pause must not `set_aec(None)`.
    tap_rms_ema: f32,
    /// Blend between raw mic (0.0) and cancelled mic (1.0), moved by
    /// `AEC_FADE_STEP` per sample toward the current target so a switch
    /// never lands a step in the recording (SOU-063).
    aec_mix: f32,
    /// `set_aec(None)` arrived while cancelled output was still being
    /// applied: keep the instance one more frame to fade it out, then drop.
    aec_retiring: bool,
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
            tap_rms_ema: 0.0,
            aec_mix: 0.0,
            aec_retiring: false,
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
        self.tap_rms_ema =
            TAP_RMS_ATTACK * mean_sq.sqrt() + (1.0 - TAP_RMS_ATTACK) * self.tap_rms_ema;
    }

    /// Enable/disable echo cancellation (e.g. when the output route changes
    /// between speakers and headphones mid-session). Passing a fresh `Aec`
    /// also resets convergence state, so its output fades in from the raw
    /// mic. Passing `None` while cancelled output is being applied keeps the
    /// old instance for the one frame it takes to fade back to raw mic.
    pub fn set_aec(&mut self, aec: Option<Aec>) {
        match aec {
            Some(aec) => {
                self.aec = Some(aec);
                self.aec_mix = 0.0;
                self.aec_retiring = false;
            }
            None if self.aec.is_some() && self.aec_mix > 0.0 => self.aec_retiring = true,
            None => {
                self.aec = None;
                self.aec_mix = 0.0;
                self.aec_retiring = false;
            }
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
        // Either way the switch is a linear ramp one frame long, not a
        // cut: the cancelled and raw signals can differ by a lot, and a
        // step at the boundary would end up in the recording.
        let target_mix = if self.tap_has_energy() && !self.aec_retiring {
            1.0
        } else {
            0.0
        };

        if n == FRAME_SAMPLES
            && let Some(aec) = self.aec.as_mut()
        {
            aec.process_render(&tap);
            let mut cancelled = mic.clone();
            aec.process_capture(&mut cancelled);

            for (raw, canc) in mic.iter_mut().zip(&cancelled) {
                if self.aec_mix < target_mix {
                    self.aec_mix = (self.aec_mix + AEC_FADE_STEP).min(target_mix);
                } else if self.aec_mix > target_mix {
                    self.aec_mix = (self.aec_mix - AEC_FADE_STEP).max(target_mix);
                }
                *raw += (canc - *raw) * self.aec_mix;
            }
            if self.aec_retiring && self.aec_mix <= 0.0 {
                self.aec = None;
                self.aec_retiring = false;
            }
        } else if self.aec_retiring {
            // Flush tail: nothing more to fade over, drop now.
            self.aec = None;
            self.aec_mix = 0.0;
            self.aec_retiring = false;
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

    /// Ceiling on the sample-to-sample step the Me leg may show when the
    /// canceller's output starts or stops being applied (SOU-063 AC3). The
    /// switch in `aec_switch_fades_instead_of_stepping` is ~0.5 between the
    /// two sources; the one-frame ramp spreads it over 480 samples.
    const AEC_SWITCH_MAX_STEP: f32 = 0.01;

    /// Engage the canceller, then retire it with `set_aec(None)`: neither
    /// boundary may land a discontinuity in the Me leg. A DC mic and a DC tap
    /// keep sonora's output predictable — its high-pass filter drives the
    /// cancelled leg to ~0 while the raw leg stays at 0.5 — so the two
    /// sources differ by ~0.5 at the switch and the fade has something to
    /// prove. No model, no real audio.
    #[test]
    fn aec_switch_fades_instead_of_stepping() {
        let (mut mic, mut tap, mut mixer) = make_mixer(48_000, 48_000, MIX_RATE);
        mixer.set_aec(Some(Aec::new(MIX_RATE)));

        // Silent far end first: the instance is fed and runs its start-up
        // transient while the blend still sits on the raw mic.
        let mut out = Vec::new();
        for _ in 0..20 {
            mic.push_slice(&vec![0.5f32; FRAME_SAMPLES]);
            out.extend(mixer.tick_split().0);
        }
        assert_eq!(mixer.aec_mix, 0.0);
        let switch_from = out.len();

        for _ in 0..40 {
            mic.push_slice(&vec![0.5f32; FRAME_SAMPLES]);
            tap.push_slice(&vec![0.3f32; FRAME_SAMPLES]);
            out.extend(mixer.tick_split().0);
        }
        assert_eq!(mixer.aec_mix, 1.0, "tap energy must have engaged the blend");
        let engaged = *out.last().unwrap();
        assert!(
            (engaged - 0.5).abs() > 0.2,
            "cancelled leg should differ from raw mic, got {engaged}"
        );

        mixer.set_aec(None);
        assert!(
            mixer.aec.is_some(),
            "instance must survive until the fade completes"
        );
        for _ in 0..5 {
            mic.push_slice(&vec![0.5f32; FRAME_SAMPLES]);
            tap.push_slice(&vec![0.3f32; FRAME_SAMPLES]);
            out.extend(mixer.tick_split().0);
        }
        assert!(mixer.aec.is_none(), "retired instance must be dropped");
        assert_eq!(mixer.aec_mix, 0.0);
        let raw = *out.last().unwrap();
        assert!(
            (raw - 0.5).abs() < 1e-3,
            "after the fade the mic must be raw, got {raw}"
        );

        let (at, max_step) = out[switch_from - 1..]
            .windows(2)
            .enumerate()
            .map(|(i, w)| (switch_from - 1 + i, (w[1] - w[0]).abs()))
            .fold((0, 0.0f32), |acc, s| if s.1 > acc.1 { s } else { acc });
        assert!(
            max_step < AEC_SWITCH_MAX_STEP,
            "switching left a {max_step} step at sample {at}"
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
/// ordering) rather than the `Aec` wrapper in isolation. All but the short
/// `broadband_echo_is_cancelled_through_the_mixer` and the measurement
/// self-check are ignored by default: several seconds of synthetic audio at
/// 48kHz is too slow for the normal test loop. Run them explicitly with:
///   cargo test --release --manifest-path src-tauri/Cargo.toml audio::mixer::aec_bench -- --ignored --nocapture
///
/// How the measurement works (SOU-112). The bench generates the user's voice
/// and the far-end render itself, so it knows exactly what the acoustic path
/// added to the mic (`echo`) and exactly what the user said (`voice`). The
/// residual after cancellation is `output - reference`, and ERLE is the
/// energy of the uncancelled echo over that residual: positive means the
/// output ended up closer to the voice than the uncancelled mic would have
/// been, 0dB means cancellation changed nothing. Four references are
/// reported side by side:
/// - `raw`: the voice at its generation index, against the raw echo. Sonora
///   delays its output by a fixed lag and its enforced high-pass filter
///   shifts the phase of every voice component (143 degrees at 120Hz, still
///   21 degrees at 1kHz); `output - voice` counts both as damage. Kept for
///   continuity with the 2026-07-18 numbers.
/// - `lag-aligned`: the voice delayed by the measured pipeline lag. Removes
///   the delay, still counts the high-pass filter's phase. This is what the
///   bench called "aligned" before SOU-112, except that its lag came from a
///   cross-correlation with the voice, capped at 240 samples and pulled by
///   the same phase shift: it reported 170 where the impulse probe finds 430.
/// - `compensated`: the voice run through a fresh `Aec` with a silent far
///   end, i.e. through the fixed part of sonora's chain (high-pass filter
///   and block delay) with nothing to cancel, against the echo run through
///   the same chain. `fixed_chain_reference_is_linear` checks that this path
///   is a plain linear filter, so the reference cannot absorb adaptive
///   behaviour. The reference follows the mixer's raw/cancelled blend frame
///   by frame, so frames where the mixer held the raw mic (a far-end pause)
///   are compared with the raw voice. What is left in `output -
///   compensated` is residual echo plus what cancellation itself did to
///   the voice.
/// - `misaligned control`: the compensated reference shifted by the measured
///   lag. If compensation measures anything, this collapses.
#[cfg(test)]
mod aec_bench {
    use std::f32::consts::TAU;

    use ringbuf::HeapRb;
    use ringbuf::traits::{Producer, Split};

    use super::*;
    use crate::audio::aec::Aec;
    use crate::audio::resampler::Resampler;

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

    /// A synthetic talker: a harmonic series on a slowly wobbling
    /// fundamental, tilted like speech, amplitude-modulated at syllable rate,
    /// with a little low-passed breath noise under it. Broadband, and a fair
    /// stand-in for the *near* end, whose exact content the bench must know.
    /// It is not a fair stand-in for the *far* end: AEC3 freezes filter
    /// adaptation when any FFT bin dominates its neighbours for more than
    /// 10 blocks (`RenderSignalAnalyzer::poor_signal_excitation`), and a
    /// sustained harmonic series does exactly that. The far end is real
    /// speech (`Render::Speech`); the synthetic one stays as an adverse
    /// diagnostic.
    struct Talker {
        /// Mean fundamental, in Hz.
        f0_hz: f32,
        /// Fractional pitch wobble and its rate in Hz.
        vibrato: (f32, f32),
        /// Syllabic amplitude modulation rate in Hz; the envelope never drops
        /// below `SYLLABLE_FLOOR`.
        syllable_hz: f32,
        /// Harmonics are generated up to this frequency ...
        max_harmonic_hz: f32,
        /// ... with this spectral tilt.
        tilt_db_per_octave: f32,
        /// Low-passed noise mixed under the harmonics.
        noise_amp: f32,
        /// RMS of the harmonic part before the syllable envelope.
        rms: f32,
        seed: u64,
    }

    const SYLLABLE_FLOOR: f32 = 0.3;

    /// Near end: the user at the mic. The fundamental sits well above the
    /// enforced high-pass filter's transition band: sonora's 48kHz HPF is
    /// flat in magnitude above ~110Hz, and the pre-SOU-112 fixture's 120Hz
    /// fundamental was already there in magnitude (-0.01dB), so what the
    /// voice-only bench charged to the HPF was its phase, not attenuation.
    /// A higher fundamental shrinks that share (120 degrees at 210Hz against
    /// 143 at 120Hz) but does not remove it; the compensated reference does.
    const NEAR_TALKER: Talker = Talker {
        f0_hz: 210.0,
        vibrato: (0.03, 5.5),
        syllable_hz: 4.0,
        max_harmonic_hz: 4000.0,
        tilt_db_per_octave: -6.0,
        noise_amp: 0.02,
        rms: 0.14,
        seed: 0xBEEF,
    };

    /// A sustained lower voice for `Render::SustainedHarmonics`. Its 125Hz
    /// fundamental lands every harmonic on one of AEC3's 125Hz FFT bins,
    /// the worst case for the poor-excitation detector.
    const FAR_TALKER: Talker = Talker {
        f0_hz: 125.0,
        vibrato: (0.04, 4.5),
        syllable_hz: 3.3,
        max_harmonic_hz: 5000.0,
        tilt_db_per_octave: -6.0,
        noise_amp: 0.05,
        rms: 0.24,
        seed: 0xC0FFEE,
    };

    fn synth_talker(len: usize, sample_rate: u32, talker: &Talker) -> Vec<f32> {
        let mut rng = Xorshift(talker.seed);
        let fs = sample_rate as f32;
        let n_harmonics = (talker.max_harmonic_hz / talker.f0_hz).floor().max(1.0) as usize;
        let amps: Vec<f32> = (1..=n_harmonics)
            .map(|k| 10f32.powf(talker.tilt_db_per_octave * (k as f32).log2() / 20.0))
            .collect();
        let norm = talker.rms / (amps.iter().map(|a| a * a).sum::<f32>() / 2.0).sqrt();
        let mut phase = 0.0f32;
        let mut noise_state = 0.0f32;
        (0..len)
            .map(|i| {
                let t = i as f32 / fs;
                let f0 =
                    talker.f0_hz * (1.0 + talker.vibrato.0 * (TAU * talker.vibrato.1 * t).sin());
                phase = (phase + TAU * f0 / fs) % TAU;
                let harmonic: f32 = amps
                    .iter()
                    .enumerate()
                    .map(|(k, a)| a * ((k + 1) as f32 * phase).sin())
                    .sum();
                let envelope = SYLLABLE_FLOOR
                    + (1.0 - SYLLABLE_FLOOR) * 0.5 * (1.0 + (TAU * talker.syllable_hz * t).sin());
                // One-pole low-pass on white noise: broadens the spectrum
                // without the harshness of raw white noise.
                noise_state = 0.9 * noise_state + 0.1 * rng.next_unit();
                (norm * harmonic * envelope + talker.noise_amp * noise_state).clamp(-1.0, 1.0)
            })
            .collect()
    }

    /// The pre-SOU-112 far-end fixture: five stationary tones standing in
    /// for video/music playback. Kept as a documented adverse case
    /// (`tonal_render_diagnostic`) so the history of measurements stays
    /// comparable; AEC3 does not adapt on it.
    const VIDEO_FREQS: &[(f32, f32)] = &[
        (220.0, 0.22),
        (523.0, 0.18),
        (1046.0, 0.14),
        (1760.0, 0.10),
        (2637.0, 0.07),
    ];

    /// Sum of sinusoids at `freqs` plus low-pass-filtered noise. With no
    /// `freqs` this is the noise-like far end of the broadband benches.
    fn synth_tones(
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
                    .map(|(freq, amp)| amp * (t * freq * TAU).sin())
                    .sum();
                noise_state = 0.9 * noise_state + 0.1 * rng.next_unit();
                (tonal + noise_amp * noise_state).clamp(-1.0, 1.0)
            })
            .collect()
    }

    /// Real speech: the SOU-030 fixtures are two French sentences from the
    /// macOS `Thomas` voice at 16kHz (f0 around 130Hz, a 300ms pause in the
    /// middle, natural consonants and level). Wideband 16kHz is also what a
    /// meeting client actually plays. Resampled to `MIX_RATE` with the
    /// production resampler and looped to `len` at its native level.
    const FAR_SPEECH_WAV: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/audio/sou-030/hesitation-b-gap0300ms.wav"
    );
    const NEAR_SPEECH_WAV: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/audio/sou-030/hesitation-a-gap0300ms.wav"
    );

    fn load_speech(path: &str, len: usize) -> Vec<f32> {
        let mut reader =
            hound::WavReader::open(path).unwrap_or_else(|e| panic!("open {path}: {e}"));
        let spec = reader.spec();
        assert_eq!(spec.channels, 1, "{path}: expected a mono fixture");
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);
        let scale = 1.0 / (1u32 << (spec.bits_per_sample - 1)) as f32;
        let pcm: Vec<f32> = reader
            .samples::<i32>()
            .map(|s| s.expect("readable sample") as f32 * scale)
            .collect();
        let mut resampler = Resampler::new(spec.sample_rate, 1, MIX_RATE, 1.0);
        let mut clip = resampler.process(&pcm);
        clip.extend(resampler.flush());
        assert!(!clip.is_empty(), "{path}: resampled to nothing");
        clip.iter().copied().cycle().take(len).collect()
    }

    /// What plays on the speakers.
    #[derive(Clone, Copy)]
    enum Render {
        /// Real speech (`FAR_SPEECH_WAV`): the meeting case.
        Speech,
        /// Low-passed noise at this level: the case AEC3 is built for.
        Noise(f32),
        /// `VIDEO_FREQS` over a little noise: the pre-SOU-112 fixture.
        Tones,
        /// `FAR_TALKER`: broadband but stationary enough to trip AEC3's
        /// poor-excitation gate.
        SustainedHarmonics,
    }

    fn synth_render(len: usize, sample_rate: u32, render: Render) -> Vec<f32> {
        match render {
            Render::Speech => load_speech(FAR_SPEECH_WAV, len),
            Render::Noise(level) => synth_tones(len, sample_rate, &[], level, 0xC0FFEE),
            Render::Tones => synth_tones(len, sample_rate, VIDEO_FREQS, 0.05, 0xC0FFEE),
            Render::SustainedHarmonics => synth_talker(len, sample_rate, &FAR_TALKER),
        }
    }

    /// What the user says into the mic.
    #[derive(Clone, Copy)]
    enum Voice {
        /// `NEAR_TALKER`, scaled by `BenchCase::voice_scale`.
        Synthetic,
        /// Real speech (`NEAR_SPEECH_WAV`), scaled likewise.
        Speech,
    }

    fn synth_voice(len: usize, sample_rate: u32, voice: Voice, scale: f32) -> Vec<f32> {
        if scale == 0.0 {
            return vec![0.0; len];
        }
        let raw = match voice {
            Voice::Synthetic => synth_talker(len, sample_rate, &NEAR_TALKER),
            Voice::Speech => load_speech(NEAR_SPEECH_WAV, len),
        };
        raw.into_iter().map(|s| s * scale).collect()
    }

    /// Frames of silence fed before probing so sonora is past its start-up
    /// state (`initial_state_seconds` in AEC3 is well under a second).
    const PROBE_WARMUP_FRAMES: usize = 100;
    /// How many frames after the impulse the probe looks for its peak. The
    /// lag is under one frame; anything beyond two is a broken pipeline.
    const PROBE_WINDOW_FRAMES: usize = 4;

    /// Sonora's response to a unit impulse through a fresh `Aec` with a
    /// silent far end, normalised to the impulse, from the impulse onwards.
    /// With nothing to cancel, this is the fixed part of its chain: the
    /// high-pass filter's impulse response (first sample 0.92 for the
    /// cascade sonora uses at 48kHz, then a small negative tail) delayed by
    /// the block-processing lag.
    fn probe_pipeline_response() -> Vec<f32> {
        let mut aec = Aec::new(MIX_RATE);
        let silence = vec![0.0f32; FRAME_SAMPLES];
        for _ in 0..PROBE_WARMUP_FRAMES {
            aec.process_render(&silence);
            let mut mic = silence.clone();
            aec.process_capture(&mut mic);
        }
        let impulse = 0.5f32;
        let mut out = Vec::with_capacity(PROBE_WINDOW_FRAMES * FRAME_SAMPLES);
        for f in 0..PROBE_WINDOW_FRAMES {
            aec.process_render(&silence);
            let mut mic = silence.clone();
            if f == 0 {
                mic[0] = impulse;
            }
            aec.process_capture(&mut mic);
            out.extend(mic.iter().map(|s| s / impulse));
        }
        out
    }

    /// Sonora's fixed output delay, measured rather than assumed: where the
    /// probe response peaks, and how tall that peak is.
    fn probe_pipeline_lag() -> (usize, f32) {
        let response = probe_pipeline_response();
        response
            .iter()
            .enumerate()
            .map(|(i, s)| (i, s.abs()))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .expect("probe window is not empty")
    }

    /// `x` through a fresh `Aec` with a silent far end, through the same
    /// wrapper and call order as `split_frame`. Sonora has nothing to
    /// cancel, so what comes out is the fixed part of its chain (the enforced
    /// high-pass filter and the block-processing delay) applied to `x`: the
    /// reference the compensated figures are measured against.
    fn fixed_chain_reference(x: &[f32]) -> Vec<f32> {
        let mut aec = Aec::new(MIX_RATE);
        let silence = vec![0.0f32; FRAME_SAMPLES];
        let mut out = Vec::with_capacity(x.len());
        for frame in x.chunks_exact(FRAME_SAMPLES) {
            aec.process_render(&silence);
            let mut mic = frame.to_vec();
            aec.process_capture(&mut mic);
            out.extend(mic);
        }
        out
    }

    /// `x` delayed by `lag` samples, zero-filled at the start.
    fn delayed(x: &[f32], lag: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; x.len()];
        let lag = lag.min(x.len());
        out[lag..].copy_from_slice(&x[..x.len() - lag]);
        out
    }

    fn energy(x: &[f32]) -> f32 {
        x.iter().map(|s| s * s).sum()
    }

    fn db(ratio: f32) -> f32 {
        10.0 * ratio.log10()
    }

    /// One synthetic run through `MeetingMixer`; see `run_echo_bench`.
    struct BenchCase {
        /// Mirrors what `Aec` passes to `set_stream_delay_ms`; `None`
        /// measures the hint-free baseline.
        expected_delay_hint_ms: Option<i32>,
        voice: Voice,
        /// Near-end voice level; `0.0` isolates echo-only cancellation.
        voice_scale: f32,
        /// Acoustic path: how much of the render leaks into the mic ...
        echo_gain: f32,
        /// ... and how late.
        delay_ms: u32,
        duration_s: f32,
        render: Render,
    }

    struct BenchResult {
        /// Sonora's fixed output delay from `probe_pipeline_lag`, and how
        /// tall the probe peak was relative to the impulse.
        lag_samples: usize,
        lag_probe_peak: f32,
        /// ERLE against the voice at its generation index: the 2026-07-18
        /// metric, which counts the pipeline lag and the high-pass filter's
        /// phase as voice damage.
        raw_erle_db: f32,
        /// ERLE against the voice delayed by `lag_samples`: the pre-SOU-112
        /// "aligned" figure, still counting the high-pass filter's phase.
        lag_aligned_erle_db: f32,
        /// ERLE of the echo through the fixed chain over `output -
        /// fixed_chain_reference(voice)`: residual echo plus what
        /// cancellation itself did to the voice, nothing else. 0dB is a
        /// canceller that changed nothing.
        compensated_erle_db: f32,
        /// Negative control: the compensated metric with its reference
        /// shifted by `lag_samples`. Must collapse relative to `compensated`.
        misaligned_erle_db: f32,
        /// What the fixed chain alone (no cancellation) does to the voice,
        /// relative to the voice, in dB: the share of the old metric that
        /// was never the canceller's doing.
        fixed_chain_cost_db: f32,
        /// Compensated residual relative to the voice itself, in dB. With no
        /// echo path this is pure voice distortion by the canceller.
        residual_vs_voice_db: f32,
        /// The same with the reference misaligned by `lag_samples`: the
        /// negative control's figure when there is no echo to normalise by.
        misaligned_residual_vs_voice_db: f32,
        /// Output level over the tail relative to the fixed-chain voice, in
        /// dB. Distortion leaves this near 0dB; suppression pulls it down.
        output_level_db: f32,
        /// Split of the compensated residual, both relative to the
        /// uncancelled echo: the part coherent with the echo frame by frame
        /// (echo the canceller left, counted fully when only scaled, partly
        /// when filtered) and everything else, which is what cancellation
        /// did to the voice plus anything nonlinear.
        residual_echo_like_db: f32,
        residual_other_db: f32,
        /// Sonora's own view after the run: estimated render/capture delay,
        /// echo return loss enhancement and divergent filter fraction.
        stats: (Option<i32>, Option<f64>, Option<f64>),
        /// First frame after which a 500ms window of compensated residual
        /// stays below -10dB of the echo in that window.
        converge_frame: Option<usize>,
        /// Mean of the mixer's raw/cancelled blend over the tail: 1.0 when
        /// the canceller's output was applied throughout, 0.0 when the
        /// mixer held the raw mic throughout.
        applied_fraction: f32,
    }

    /// Feeds a "remote participant on the speakers while the user talks"
    /// scenario through `MeetingMixer` and reports (does not just assert)
    /// how effectively the AEC integration attenuates the echo: ERLE
    /// post-convergence under each reference of `BenchResult`, and roughly
    /// when convergence happens.
    fn run_echo_bench(case: BenchCase) -> BenchResult {
        let sample_rate = MIX_RATE;
        let delay_samples = (sample_rate as usize * case.delay_ms as usize) / 1000;
        let n_frames = (sample_rate as f32 * case.duration_s) as usize / FRAME_SAMPLES;
        let total_samples = n_frames * FRAME_SAMPLES;

        let render = synth_render(total_samples, sample_rate, case.render);
        let voice = synth_voice(total_samples, sample_rate, case.voice, case.voice_scale);

        // Ground truth: exactly what the acoustic path adds to the mic and
        // exactly what the user said.
        let echo: Vec<f32> = (0..total_samples)
            .map(|n| {
                if n >= delay_samples {
                    render[n - delay_samples] * case.echo_gain
                } else {
                    0.0
                }
            })
            .collect();
        let capture: Vec<f32> = (0..total_samples)
            .map(|n| (voice[n] + echo[n]).clamp(-1.0, 1.0))
            .collect();

        let (lag_samples, lag_probe_peak) = probe_pipeline_lag();
        let voice_ref = fixed_chain_reference(&voice);
        let echo_ref = fixed_chain_reference(&echo);

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
        if let Some(hint) = case.expected_delay_hint_ms {
            aec.set_expected_delay_ms(hint);
        }
        mixer.set_aec(Some(aec));

        let mut output = Vec::with_capacity(total_samples);
        // The mixer's raw/cancelled blend at the end of each frame. The
        // mixer holds the raw mic while the tap carries no energy (SOU-063
        // lever 2), which a speech far end does in every pause, and the
        // frames where it did must be compared with the raw voice (no lag,
        // no high-pass filter), not with the fixed chain.
        let mut blend = Vec::with_capacity(n_frames);
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
            output.extend(me);
            blend.push(mixer.aec_mix);
        }

        // Post-convergence window: last quarter of the run.
        let tail = n_frames / 4;
        let tail_start = (n_frames - tail) * FRAME_SAMPLES;

        // References that follow the path the mixer actually took, frame by
        // frame. The one-frame ramp inside a switching frame is not
        // modelled, so a switch costs a small error in that frame only.
        let blended = |raw: &[f32], through_chain: &[f32]| -> Vec<f32> {
            (0..total_samples)
                .map(|n| {
                    let mix = blend[n / FRAME_SAMPLES];
                    raw[n] + (through_chain[n] - raw[n]) * mix
                })
                .collect()
        };
        let voice_ref = blended(&voice, &voice_ref);
        let echo_ref = blended(&echo, &echo_ref);
        let applied_fraction = blend[n_frames - tail..].iter().sum::<f32>() / tail.max(1) as f32;

        let residual_energy = |from: usize, to: usize, reference: &[f32]| -> f32 {
            (from..to).map(|n| (output[n] - reference[n]).powi(2)).sum()
        };
        let erle_db = |uncancelled: &[f32], reference: &[f32]| -> f32 {
            let pre = energy(&uncancelled[tail_start..]);
            let post = residual_energy(tail_start, total_samples, reference);
            db(pre / post.max(1e-9))
        };
        let raw_erle_db = erle_db(&echo, &voice);
        let lag_aligned_voice = delayed(&voice, lag_samples);
        let lag_aligned_erle_db = erle_db(&echo, &lag_aligned_voice);
        let compensated_erle_db = erle_db(&echo_ref, &voice_ref);
        let misaligned_voice_ref = delayed(&voice_ref, lag_samples);
        let misaligned_erle_db = erle_db(&echo_ref, &misaligned_voice_ref);

        let voice_energy = energy(&voice[tail_start..]).max(1e-9);
        let fixed_chain_cost_db = db((tail_start..total_samples)
            .map(|n| (voice_ref[n] - lag_aligned_voice[n]).powi(2))
            .sum::<f32>()
            / voice_energy);
        let residual_vs_voice_db =
            db(residual_energy(tail_start, total_samples, &voice_ref) / voice_energy);
        let misaligned_residual_vs_voice_db =
            db(residual_energy(tail_start, total_samples, &misaligned_voice_ref) / voice_energy);
        let output_level_db =
            db(energy(&output[tail_start..]) / energy(&voice_ref[tail_start..]).max(1e-9));

        // Per-frame scalar projection of the compensated residual onto the
        // uncancelled echo: a time-varying gain is allowed, spectral
        // colouring is not, so `echo_like` is a floor on the echo left.
        let (mut echo_like, mut residual_total) = (0.0f32, 0.0f32);
        for f in n_frames - tail..n_frames {
            let (start, end) = (f * FRAME_SAMPLES, (f + 1) * FRAME_SAMPLES);
            let residual: Vec<f32> = (start..end).map(|n| output[n] - voice_ref[n]).collect();
            let echo_energy = energy(&echo_ref[start..end]);
            if echo_energy > 1e-12 {
                let dot: f32 = residual
                    .iter()
                    .zip(&echo_ref[start..end])
                    .map(|(r, e)| r * e)
                    .sum();
                echo_like += dot * dot / echo_energy;
            }
            residual_total += energy(&residual);
        }
        let uncancelled = energy(&echo_ref[tail_start..]).max(1e-9);
        let residual_echo_like_db = db(echo_like / uncancelled);
        let residual_other_db = db((residual_total - echo_like).max(0.0) / uncancelled);

        let stats = mixer
            .aec
            .as_ref()
            .map(|aec| {
                let s = aec.statistics();
                (
                    s.delay_ms,
                    s.echo_return_loss_enhancement,
                    s.divergent_filter_fraction,
                )
            })
            .unwrap_or_default();

        // Convergence point: first frame after which a 500ms sliding window
        // of compensated residual energy stays below 10% (-10dB) of the
        // uncancelled echo energy in that same window.
        let sustain_frames = 50;
        let per_frame: Vec<(f32, f32)> = (0..n_frames)
            .map(|f| {
                let start = f * FRAME_SAMPLES;
                let end = start + FRAME_SAMPLES;
                (
                    energy(&echo_ref[start..end]),
                    residual_energy(start, end, &voice_ref),
                )
            })
            .collect();
        let converge_frame = (0..n_frames.saturating_sub(sustain_frames)).find(|&i| {
            let (window_pre, window_post) = per_frame[i..i + sustain_frames]
                .iter()
                .fold((0.0f32, 0.0f32), |acc, (e, r)| (acc.0 + e, acc.1 + r));
            window_post < window_pre * 0.1
        });

        BenchResult {
            lag_samples,
            lag_probe_peak,
            raw_erle_db,
            lag_aligned_erle_db,
            compensated_erle_db,
            misaligned_erle_db,
            fixed_chain_cost_db,
            residual_vs_voice_db,
            misaligned_residual_vs_voice_db,
            output_level_db,
            residual_echo_like_db,
            residual_other_db,
            stats,
            converge_frame,
            applied_fraction,
        }
    }

    fn report(label: &str, r: &BenchResult) {
        println!(
            "AEC bench ({label}):\n  \
             pipeline lag = {} samples ({:.1} ms), probe peak = {:.2}\n  \
             ERLE post-convergence: raw = {:.1} dB | lag-aligned = {:.1} dB | \
             compensated = {:.1} dB | misaligned control = {:.1} dB\n  \
             fixed chain vs voice = {:.1} dB, residual vs voice = {:.1} dB \
             (misaligned: {:.1} dB), output level vs voice = {:.1} dB, convergence at ~{}\n  \
             residual vs uncancelled echo: echo-like = {:.1} dB, other (voice damage) = {:.1} dB\n  \
             cancelled output applied over {:.0}% of the tail, \
             sonora: delay={:?}ms erle={:?} divergent={:?}",
            r.lag_samples,
            r.lag_samples as f32 * 1000.0 / MIX_RATE as f32,
            r.lag_probe_peak,
            r.raw_erle_db,
            r.lag_aligned_erle_db,
            r.compensated_erle_db,
            r.misaligned_erle_db,
            r.fixed_chain_cost_db,
            r.residual_vs_voice_db,
            r.misaligned_residual_vs_voice_db,
            r.output_level_db,
            r.converge_frame
                .map(|f| format!("{}ms", f * 10))
                .unwrap_or_else(|| "never".to_string()),
            r.residual_echo_like_db,
            r.residual_other_db,
            r.applied_fraction * 100.0,
            r.stats.0,
            r.stats.1,
            r.stats.2,
        );
    }

    // Target for the double-talk benches (SOU-063 AC1): the output must end
    // up closer to the user's voice than the uncancelled echo would have
    // left it, so the compensated ERLE must be positive. Not met; the
    // double-talk benches fail when run explicitly and print what they got.
    //
    // Measured 2026-09-09 (release build, `build_apm` at 48kHz, SOU-112
    // measurement), compensated unless stated. The mixer holds the raw mic
    // in the far end's pauses (14% of the tail on the speech fixture), and
    // the reference follows it; without that the same runs read -4.0dB and
    // -10.7dB, i.e. the old metric was still charging the canceller for the
    // lag and filter of frames it never touched.
    // - speech far end, synthetic near end: +2.2dB (-7.5dB lag-aligned,
    //   -9.7dB raw) with the 50ms `stream_delay_ms` hint, +2.4dB without.
    //   The canceller takes 8.8dB off the echo and puts 3.3dB less than the
    //   echo's energy into the voice; sonora's own ERLE stays at 2.7dB and
    //   its delay estimate at 32ms for a 50ms path, because the near end
    //   never pauses. Marginal: hints of 0, 80 or 120ms make it negative.
    // - real speech both sides: -6.5dB. Sonora finds the delay (48ms) and
    //   converges in the near end's pauses, then during double talk takes
    //   only 2.9dB off the echo and puts 6.0dB *more* than the echo's
    //   energy into the voice (residual -0.9dB of the voice at an almost
    //   unchanged level: distortion, not gain).
    // - echo only, speech far end: +69dB. Cancellation itself works.
    // - the pre-SOU-112 tonal fixture: 0.0dB. AEC3 never adapted on it, so
    //   its -3.6dB "aligned" / -9.9dB raw were the high-pass filter's phase
    //   and a mis-measured lag, not the canceller. On the far end it does
    //   adapt on, the realistic double-talk case is still negative.
    const DOUBLE_TALK_ERLE_TARGET_DB: f32 = 0.0;

    fn double_talk(expected_delay_hint_ms: Option<i32>) -> BenchCase {
        BenchCase {
            expected_delay_hint_ms,
            voice: Voice::Synthetic,
            voice_scale: 1.0,
            echo_gain: 0.3,
            delay_ms: 50,
            duration_s: 8.0,
            render: Render::Speech,
        }
    }

    fn assert_double_talk_target(r: &BenchResult) {
        assert!(
            r.compensated_erle_db > DOUBLE_TALK_ERLE_TARGET_DB,
            "double-talk output is further from the voice than the echo was: {:.1} dB \
             compensated ({:.1} dB lag-aligned, {:.1} dB raw)",
            r.compensated_erle_db,
            r.lag_aligned_erle_db,
            r.raw_erle_db
        );
    }

    /// Regression floor for `broadband_echo_is_cancelled_through_the_mixer`:
    /// measured 38.9dB on 8s, see the test for the 3s figure. Anything under
    /// this means the render/capture plumbing through `split_frame` broke,
    /// not that AEC3 got marginally worse.
    const BROADBAND_ECHO_ERLE_FLOOR_DB: f32 = 20.0;

    /// The one AEC bench in the default test loop (SOU-063 AC2). Echo only,
    /// noise-like far end, 3s: the case AEC3 is built for, run through the
    /// real mixer integration rather than the `Aec` wrapper alone. ~1s in a
    /// debug build.
    #[test]
    fn broadband_echo_is_cancelled_through_the_mixer() {
        let r = run_echo_bench(BenchCase {
            voice_scale: 0.0,
            duration_s: 3.0,
            render: Render::Noise(0.4),
            ..double_talk(Some(50))
        });
        report("echo only, broadband render, 3s, 50ms hint", &r);
        assert!(
            r.compensated_erle_db > BROADBAND_ECHO_ERLE_FLOOR_DB,
            "broadband echo cancellation regressed: got {:.1} dB",
            r.compensated_erle_db
        );
    }

    /// Ceiling on how far the silent-far-end path may be from additive
    /// before the compensated reference stops being trustworthy. -40dB
    /// leaves room for float rounding and AEC3's -96dBFS comfort-noise
    /// floor; anything adaptive would show up tens of dB higher.
    const REFERENCE_LINEARITY_CEILING_DB: f32 = -40.0;

    /// Measurement self-check, in the default loop because the compensated
    /// figure is only honest if its reference is a fixed linear filter. The
    /// metric relies on `chain(voice + echo) == chain(voice) + chain(echo)`,
    /// so that is what is checked, on two unrelated signals; and the impulse
    /// probe must find a lag that looks like a delayed high-pass response.
    #[test]
    fn fixed_chain_reference_is_linear() {
        let response = probe_pipeline_response();
        let (lag, peak) = probe_pipeline_lag();
        assert!(lag > 0, "the pipeline lag probe found no delay at all");
        assert!(
            lag < FRAME_SAMPLES * 2,
            "pipeline lag {lag} samples is beyond two frames; the probe is not seeing the impulse"
        );
        assert!(
            (0.8..=1.0).contains(&peak),
            "probe peak {peak:.2} does not look like a high-pass impulse response (~0.92)"
        );
        let before = &response[lag.saturating_sub(2)..lag];
        let after = &response[lag..(lag + 4).min(response.len())];

        let len = MIX_RATE as usize; // 1s
        let a = synth_talker(len, MIX_RATE, &NEAR_TALKER);
        let b = synth_talker(len, MIX_RATE, &FAR_TALKER);
        let sum: Vec<f32> = a.iter().zip(&b).map(|(x, y)| x + y).collect();
        let chain_a = fixed_chain_reference(&a);
        let chain_b = fixed_chain_reference(&b);
        let chain_sum = fixed_chain_reference(&sum);
        // Skip the first quarter: sonora's start-up state may not be linear.
        let from = len / 4;
        let deviation: f32 = (from..len)
            .map(|n| (chain_sum[n] - chain_a[n] - chain_b[n]).powi(2))
            .sum();
        let deviation_db = db(deviation / energy(&chain_sum[from..]).max(1e-9));
        println!(
            "AEC bench reference: lag = {lag} samples, probe response {before:.3?} | \
             {after:.3?}, additivity deviation = {deviation_db:.1} dB"
        );
        assert!(
            deviation_db < REFERENCE_LINEARITY_CEILING_DB,
            "the silent-far-end path is not additive ({deviation_db:.1} dB): it is doing \
             something adaptive and cannot serve as the compensated reference"
        );
    }

    #[test]
    #[ignore = "multi-second synthetic bench, run explicitly with -- --ignored --nocapture"]
    fn double_talk_baseline() {
        let r = run_echo_bench(double_talk(None));
        report(
            "double talk, speech render, no stream_delay_ms hint, 50ms acoustic delay",
            &r,
        );
        assert_double_talk_target(&r);
    }

    #[test]
    #[ignore = "multi-second synthetic bench, run explicitly with -- --ignored --nocapture"]
    fn double_talk_with_delay_hint() {
        let r = run_echo_bench(double_talk(Some(50)));
        report(
            "double talk, speech render, 50ms stream_delay_ms hint, 50ms acoustic delay",
            &r,
        );
        assert_double_talk_target(&r);
    }

    /// How closely the mixer's Me leg must track the fixed-chain reference
    /// when the canceller has nothing to do, and how far the misaligned
    /// control must fall from there. A one-sample misalignment of 4kHz
    /// content already costs tens of dB, so 20dB is a loose bound on the
    /// collapse; -30dB is a loose bound on the match (measured -54dB).
    const CONTROL_MATCH_CEILING_DB: f32 = -30.0;
    const CONTROL_MIN_COLLAPSE_DB: f32 = 20.0;

    /// Negative control (SOU-112): validates the ruler on a known object.
    /// With no echo path and a far end AEC3 will not adapt on (its
    /// poor-excitation gate freezes on `SustainedHarmonics`), the canceller
    /// is transparent and the Me leg through `split_frame` must equal the
    /// fixed-chain reference sample for sample, which is only true if the
    /// reference has the same lag and the same filter as the real path.
    /// Reintroducing the measured pipeline lag into that reference must
    /// then collapse the figure. On the double-talk case the same shift is
    /// reported but not asserted: once the canceller has suppressed the
    /// voice, misaligning the reference changes little, and that is a fact
    /// about the canceller, not about the measurement.
    #[test]
    #[ignore = "multi-second synthetic bench, run explicitly with -- --ignored --nocapture"]
    fn misaligned_reference_collapses_compensated_erle() {
        let r = run_echo_bench(BenchCase {
            echo_gain: 0.0,
            render: Render::SustainedHarmonics,
            ..double_talk(Some(50))
        });
        report(
            "negative control, voice only, sustained harmonic render, no echo path",
            &r,
        );
        assert!(
            r.residual_vs_voice_db < CONTROL_MATCH_CEILING_DB,
            "with nothing to cancel the Me leg should equal the fixed-chain reference, but \
             differs by {:.1} dB of the voice: the reference has a different lag or filter \
             than the real path",
            r.residual_vs_voice_db
        );
        let collapse = r.misaligned_residual_vs_voice_db - r.residual_vs_voice_db;
        assert!(
            collapse > CONTROL_MIN_COLLAPSE_DB,
            "shifting the reference by the {}-sample pipeline lag only moved the residual by \
             {collapse:.1} dB; the compensation is not measuring alignment",
            r.lag_samples
        );

        let r = run_echo_bench(double_talk(Some(50)));
        report(
            "negative control, double talk, speech render, 50ms hint",
            &r,
        );
        println!(
            "  misaligning the reference moves the double-talk ERLE by {:.1} dB",
            r.compensated_erle_db - r.misaligned_erle_db
        );
    }

    /// Real speech on both sides (the user reads one SOU-030 sentence while
    /// the other plays on the speakers): the most realistic double-talk
    /// case, held to the same target. The near-end voice has a ~130Hz
    /// fundamental inside the high-pass filter's phase shift, which the
    /// compensated reference makes irrelevant (`fixed chain vs voice` shows
    /// what the raw metric would have charged for it); its pauses also give
    /// AEC3 near-end-free windows to adapt in, which the continuous
    /// synthetic near end of `double_talk_with_delay_hint` does not.
    #[test]
    #[ignore = "multi-second synthetic bench, run explicitly with -- --ignored --nocapture"]
    fn double_talk_real_speech_both_sides() {
        let r = run_echo_bench(BenchCase {
            voice: Voice::Speech,
            ..double_talk(Some(50))
        });
        report("double talk, real speech near and far, 50ms hint", &r);
        assert_double_talk_target(&r);
    }

    /// Diagnostic: the double-talk scenario with a noise-like far end, to
    /// separate what AEC3 can do from what a speech far end lets it do.
    #[test]
    #[ignore = "multi-second synthetic bench, run explicitly with -- --ignored --nocapture"]
    fn double_talk_broadband_render_diagnostic() {
        let r = run_echo_bench(BenchCase {
            render: Render::Noise(0.4),
            ..double_talk(Some(50))
        });
        report("double talk, broadband noise render, 50ms hint", &r);
    }

    /// Diagnostic: the pre-SOU-112 fixture (five stationary tones on the
    /// speakers), kept as the known adverse case. Measured -3.6dB
    /// lag-aligned / -9.9dB raw on 2026-09-09 with sonora's delay estimate
    /// stuck at 0ms: AEC3 does not adapt on a narrowband, stationary render.
    #[test]
    #[ignore = "multi-second synthetic bench, run explicitly with -- --ignored --nocapture"]
    fn tonal_render_diagnostic() {
        let r = run_echo_bench(BenchCase {
            render: Render::Tones,
            ..double_talk(Some(50))
        });
        report(
            "double talk, tonal render (pre-SOU-112 fixture), 50ms hint",
            &r,
        );
    }

    /// Diagnostic: a broadband but sustained harmonic far end. Every
    /// harmonic sits on one of AEC3's FFT bins, so its poor-excitation gate
    /// freezes adaptation: broadband is not enough, the render has to move
    /// the way speech does.
    #[test]
    #[ignore = "multi-second synthetic bench, run explicitly with -- --ignored --nocapture"]
    fn sustained_harmonics_render_diagnostic() {
        let r = run_echo_bench(BenchCase {
            render: Render::SustainedHarmonics,
            ..double_talk(Some(50))
        });
        report("double talk, sustained harmonic render, 50ms hint", &r);
        let r = run_echo_bench(BenchCase {
            voice_scale: 0.0,
            render: Render::SustainedHarmonics,
            ..double_talk(Some(50))
        });
        report("echo only, sustained harmonic render, 50ms hint", &r);
    }

    /// Diagnostic (not a hard requirement): isolates echo-only cancellation
    /// (no near-end voice) on the speech far end through the exact same
    /// mixer integration, to tell whether a poor double-talk result comes
    /// from echo cancellation itself or from the double-talk interaction.
    #[test]
    #[ignore = "multi-second synthetic bench, run explicitly with -- --ignored --nocapture"]
    fn echo_only_speech_render_diagnostic() {
        let r = run_echo_bench(BenchCase {
            voice_scale: 0.0,
            ..double_talk(Some(50))
        });
        report(
            "echo only, speech render, 50ms hint, 50ms acoustic delay",
            &r,
        );
    }

    /// Diagnostic: voice only, render playing but no acoustic coupling
    /// (headphones-like). There is no echo to remove, so the compensated
    /// residual is purely what the canceller does to a voice it should pass
    /// through untouched, and `fixed chain vs voice` is what the enforced
    /// high-pass filter alone costs the raw metric.
    #[test]
    #[ignore = "multi-second synthetic bench, run explicitly with -- --ignored --nocapture"]
    fn voice_passthrough_diagnostic() {
        let r = run_echo_bench(BenchCase {
            echo_gain: 0.0,
            ..double_talk(Some(50))
        });
        report("voice only, speech render, no echo path, 50ms hint", &r);
        let r = run_echo_bench(BenchCase {
            echo_gain: 0.0,
            render: Render::Noise(0.4),
            ..double_talk(Some(50))
        });
        report(
            "voice only, broadband noise render, no echo path, 50ms hint",
            &r,
        );
    }
}
