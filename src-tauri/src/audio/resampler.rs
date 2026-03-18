use rubato::{FftFixedInOut, Resampler as RubatoResampler};

const TARGET_SAMPLE_RATE: usize = 16_000;

/// Wraps rubato for high-quality resampling to 16kHz mono f32
pub struct Resampler {
    resampler: Option<FftFixedInOut<f32>>,
    source_channels: usize,
}

impl Resampler {
    pub fn new(source_rate: u32, source_channels: u16) -> Self {
        let source_rate = source_rate as usize;
        let source_channels = source_channels as usize;

        let resampler = if source_rate != TARGET_SAMPLE_RATE {
            // chunk_size must divide evenly — rubato will pick an appropriate size
            FftFixedInOut::new(source_rate, TARGET_SAMPLE_RATE, 1024, 1)
                .ok()
        } else {
            None
        };

        Self {
            resampler,
            source_channels,
        }
    }

    /// Convert interleaved multi-channel samples to mono 16kHz f32
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

        // Step 2: resample if needed
        if let Some(ref mut resampler) = self.resampler {
            let frames_needed = resampler.input_frames_next();
            let mut output = Vec::new();

            for chunk in mono.chunks(frames_needed) {
                if chunk.len() < frames_needed {
                    // pad the last chunk with zeros
                    let mut padded = chunk.to_vec();
                    padded.resize(frames_needed, 0.0);
                    if let Ok(result) = resampler.process(&[padded], None) {
                        output.extend_from_slice(&result[0]);
                    }
                } else {
                    if let Ok(result) = resampler.process(&[chunk.to_vec()], None) {
                        output.extend_from_slice(&result[0]);
                    }
                }
            }

            output
        } else {
            mono
        }
    }
}
