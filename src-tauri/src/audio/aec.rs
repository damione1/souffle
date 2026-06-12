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

pub struct Aec {
    apm: AudioProcessing,
    /// 10ms at the configured sample rate.
    frame_len: usize,
    render_out: Vec<f32>,
}

impl Aec {
    pub fn new(sample_rate: u32) -> Self {
        let stream = StreamConfig::new(sample_rate, 1);
        let apm = AudioProcessing::builder()
            .config(Config {
                echo_canceller: Some(EchoCanceller::default()),
                ..Config::default()
            })
            .capture_config(stream)
            .render_config(stream)
            .build();
        let frame_len = sample_rate as usize / 100;
        Self {
            apm,
            frame_len,
            render_out: vec![0.0; frame_len],
        }
    }

    /// Feed a 10ms far-end frame (the system audio about to leave the
    /// speakers). Must be called before the capture frame of the same tick.
    pub fn process_render(&mut self, frame: &[f32]) {
        debug_assert_eq!(frame.len(), self.frame_len);
        let _ = self
            .apm
            .process_render_f32(&[frame], &mut [&mut self.render_out]);
    }

    /// Remove the echo of previously-rendered audio from a 10ms mic frame,
    /// in place.
    pub fn process_capture(&mut self, frame: &mut [f32]) {
        debug_assert_eq!(frame.len(), self.frame_len);
        let mut out = vec![0.0; self.frame_len];
        if self
            .apm
            .process_capture_f32(&[frame], &mut [&mut out])
            .is_ok()
        {
            frame.copy_from_slice(&out);
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
                    if idx >= delay { tone[idx - delay] * 0.3 } else { 0.0 }
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
}
