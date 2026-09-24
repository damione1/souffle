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

            let mut output = Vec::new();

            // Only process complete chunks — no zero-padding. Walk the
            // buffer with a cursor and drop the consumed prefix once: a
            // `drain(..chunk)` per chunk shifts the whole remaining buffer
            // every time, which is quadratic in the input length - harmless
            // for the live capture path's small blocks, but 22 s to resample
            // a 6-minute recording for playback (SOU-258).
            let mut consumed = 0;
            loop {
                let frames_needed = resampler.input_frames_next();
                if self.input_buffer.len() - consumed < frames_needed {
                    break;
                }
                let chunk = &self.input_buffer[consumed..consumed + frames_needed];
                if let Ok(result) = resampler.process(&[chunk], None)
                    && let Some(channel) = result.first()
                {
                    output.extend_from_slice(channel);
                }
                consumed += frames_needed;
            }
            // Remaining samples stay in input_buffer for the next call
            self.input_buffer.drain(..consumed);

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

    /// Flush the remaining partial chunk by zero-padding it to a full FFT
    /// frame and resampling once. Called when capture stops so the last
    /// spoken samples are not discarded; the zero tail is harmless silence.
    pub fn flush(&mut self) -> Vec<f32> {
        let Some(ref mut resampler) = self.resampler else {
            return std::mem::take(&mut self.input_buffer);
        };
        if self.input_buffer.is_empty() {
            return Vec::new();
        }

        let frames_needed = resampler.input_frames_next();
        let mut chunk = std::mem::take(&mut self.input_buffer);
        chunk.resize(frames_needed, 0.0);

        let mut output = match resampler.process(&[chunk], None) {
            Ok(result) => match result.first() {
                Some(channel) => channel.clone(),
                None => return Vec::new(),
            },
            Err(_) => return Vec::new(),
        };

        if self.gain != 1.0 {
            for sample in &mut output {
                *sample = (*sample * self.gain).clamp(-1.0, 1.0);
            }
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(len: usize) -> Vec<f32> {
        (0..len).map(|i| ((i % 480) as f32 / 480.0) - 0.5).collect()
    }

    #[test]
    fn one_large_block_matches_many_small_ones() {
        // The playback path hands a whole recording in one call; the live
        // capture path hands small blocks. Both must produce the same audio.
        let input = ramp(48_000 * 3 + 123);
        let mut whole = Resampler::new(48_000, 1, 44_100, 1.0);
        let mut out_whole = whole.process(&input);
        out_whole.extend(whole.flush());

        let mut blocks = Resampler::new(48_000, 1, 44_100, 1.0);
        let mut out_blocks = Vec::new();
        for block in input.chunks(512) {
            out_blocks.extend(blocks.process(block));
        }
        out_blocks.extend(blocks.flush());

        assert_eq!(out_whole, out_blocks);
        // 3 s at 44.1 kHz, give or take the zero-padded final FFT frame.
        assert!(out_whole.len() >= 44_100 * 3);
        assert!(out_whole.len() < 44_100 * 3 + 2048);
    }

    #[test]
    fn keeps_the_incomplete_tail_for_the_next_call() {
        let mut resampler = Resampler::new(48_000, 1, 24_000, 1.0);
        let chunk = resampler
            .resampler
            .as_ref()
            .map(|r| r.input_frames_next())
            .unwrap();
        assert!(resampler.process(&ramp(chunk - 1)).is_empty());
        assert_eq!(resampler.input_buffer.len(), chunk - 1);
        assert!(!resampler.process(&ramp(1)).is_empty());
        assert!(resampler.input_buffer.is_empty());
    }
}
