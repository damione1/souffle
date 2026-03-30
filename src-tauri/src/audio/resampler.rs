use rubato::{FftFixedInOut, Resampler as RubatoResampler};
use tracing::{info, warn};

/// Wraps rubato for high-quality resampling to the engine's expected
/// sample rate as mono f32, with a configurable gain for live mic input.
///
/// Buffers input samples internally so the FFT resampler always
/// receives complete chunks (no zero-padding artifacts).
pub struct Resampler {
    resampler: Option<FftFixedInOut<f32>>,
    source_channels: usize,
    /// Accumulation buffer for incomplete chunks
    input_buffer: Vec<f32>,
    /// Gain factor applied after resampling (engine-dependent)
    gain: f32,
}

impl Resampler {
    pub fn new(source_rate: u32, source_channels: u16, target_rate: u32, gain: f32) -> Self {
        let source_rate = source_rate as usize;
        let target_rate = target_rate as usize;
        let source_channels = source_channels as usize;

        let resampler = if source_rate != target_rate {
            match FftFixedInOut::new(source_rate, target_rate, 1024, 1) {
                Ok(r) => {
                    info!(
                        "Resampler created: {}Hz → {}Hz, chunk_in={}, chunk_out={}",
                        source_rate,
                        target_rate,
                        r.input_frames_next(),
                        r.output_frames_next()
                    );
                    Some(r)
                }
                Err(e) => {
                    warn!("Resampler creation failed: {e}");
                    warn!("Audio will NOT be resampled — model expects {target_rate}Hz!");
                    None
                }
            }
        } else {
            info!("Source rate matches target ({target_rate}Hz), no resampling needed");
            None
        };

        Self {
            resampler,
            source_channels,
            input_buffer: Vec::new(),
            gain,
        }
    }

    /// Convert interleaved multi-channel samples to mono 24kHz f32 with gain
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        // Step 1: downmix to mono
        let mono = if self.source_channels > 1 {
            input
                .chunks_exact(self.source_channels)
                .map(|frame| frame.iter().sum::<f32>() / self.source_channels as f32)
                .collect::<Vec<f32>>()
        } else {
            input.to_vec()
        };

        // Step 2: resample if needed (with proper buffering)
        let mut resampled = if let Some(ref mut resampler) = self.resampler {
            // Accumulate samples in the input buffer
            self.input_buffer.extend_from_slice(&mono);

            let frames_needed = resampler.input_frames_next();
            let mut output = Vec::new();

            // Only process complete chunks — no zero-padding
            while self.input_buffer.len() >= frames_needed {
                let chunk: Vec<f32> = self.input_buffer.drain(..frames_needed).collect();
                if let Ok(result) = resampler.process(&[chunk], None) {
                    output.extend_from_slice(&result[0]);
                }
            }
            // Remaining samples stay in input_buffer for the next call

            output
        } else {
            mono
        };

        // Step 3: apply engine-specific gain and hard-clip to [-1.0, 1.0]
        if self.gain != 1.0 {
            for sample in &mut resampled {
                *sample = (*sample * self.gain).clamp(-1.0, 1.0);
            }
        }

        resampled
    }
}
