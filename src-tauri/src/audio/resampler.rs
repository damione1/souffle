use rubato::{FftFixedInOut, Resampler as RubatoResampler};

/// Kyutai Mimi codec expects 24kHz audio input
const TARGET_SAMPLE_RATE: usize = 24_000;

/// Fixed gain applied to microphone input.
/// Live mic capture typically produces -30dBFS peaks (max_amp ~0.03).
/// The Kyutai model was trained with -24dB to +15dB augmentation,
/// so we need to boost by ~15x to reach the model's expected range.
const MIC_GAIN: f32 = 15.0;

/// Wraps rubato for high-quality resampling to 24kHz mono f32,
/// with a fixed gain boost for live mic input.
///
/// Buffers input samples internally so the FFT resampler always
/// receives complete chunks (no zero-padding artifacts).
pub struct Resampler {
    resampler: Option<FftFixedInOut<f32>>,
    source_channels: usize,
    /// Accumulation buffer for incomplete chunks
    input_buffer: Vec<f32>,
}

impl Resampler {
    pub fn new(source_rate: u32, source_channels: u16) -> Self {
        let source_rate = source_rate as usize;
        let source_channels = source_channels as usize;

        let resampler = if source_rate != TARGET_SAMPLE_RATE {
            match FftFixedInOut::new(source_rate, TARGET_SAMPLE_RATE, 1024, 1) {
                Ok(r) => {
                    eprintln!(
                        "[souffle] Resampler created: {}Hz → {}Hz, chunk_in={}, chunk_out={}",
                        source_rate, TARGET_SAMPLE_RATE,
                        r.input_frames_next(), r.output_frames_next()
                    );
                    Some(r)
                }
                Err(e) => {
                    eprintln!("[souffle] WARNING: Resampler creation failed: {e}");
                    eprintln!("[souffle] Audio will NOT be resampled — model expects 24kHz!");
                    None
                }
            }
        } else {
            eprintln!("[souffle] Source rate matches target (24kHz), no resampling needed");
            None
        };

        Self {
            resampler,
            source_channels,
            input_buffer: Vec::new(),
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

        // Step 3: apply fixed gain and hard-clip to [-1.0, 1.0]
        for sample in &mut resampled {
            *sample = (*sample * MIC_GAIN).clamp(-1.0, 1.0);
        }

        resampled
    }
}
