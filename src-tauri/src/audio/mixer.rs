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

        for (sample, tap) in mic.iter_mut().zip(&tap) {
            *sample = (*sample + *tap).clamp(-1.0, 1.0);
        }
        mic
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
    ) -> (
        ringbuf::HeapProd<f32>,
        ringbuf::HeapProd<f32>,
        MeetingMixer,
    ) {
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

        assert!(mixer.tap_discarded() > 0, "excess tap lead should be dropped");
        assert!(mixer.tap_fifo.len() <= MAX_TAP_LEAD_SAMPLES + FRAME_SAMPLES);
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
