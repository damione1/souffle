use std::path::Path;

use tracing::debug;
use vad_rs::Vad;

use super::{AudioFilter, AudioFilterKind};

/// Silero VAD v4 via vad-rs (file-based ONNX model loading).
/// VAD sample rate: Silero expects 16kHz.
const VAD_SAMPLE_RATE: u32 = 16_000;

/// Frame size: 30ms at 16kHz = 480 samples (Silero v4 native frame).
const VAD_FRAME_SAMPLES: usize = (VAD_SAMPLE_RATE * 30 / 1000) as usize;

/// Speech probability threshold (0.0–1.0).
const SPEECH_THRESHOLD: f32 = 0.3;

/// Hangover frames: keep forwarding audio this many frames after speech ends.
/// At 30ms per frame, 15 frames = 450ms of post-speech tail.
const HANGOVER_FRAMES: u32 = 15;

/// Onset frames: require this many consecutive voice frames to trigger speech.
/// Prevents single-frame noise spikes from being classified as speech.
const ONSET_FRAMES: u32 = 2;

pub struct SileroVadFilter {
    engine: Vad,
    source_sample_rate: u32,
    buffer: Vec<f32>,
    hangover_remaining: u32,
    onset_count: u32,
    in_speech: bool,
}

impl SileroVadFilter {
    pub fn new(model_path: &Path, source_sample_rate: u32) -> Result<Self, String> {
        let engine = Vad::new(model_path, VAD_SAMPLE_RATE as usize)
            .map_err(|e| format!("Silero VAD init: {e}"))?;

        Ok(Self {
            engine,
            source_sample_rate,
            buffer: Vec::new(),
            hangover_remaining: 0,
            onset_count: 0,
            in_speech: false,
        })
    }

    /// Cheap 3:2 decimation from 24kHz to 16kHz (drop every 3rd sample).
    fn decimate_24k_to_16k(samples: &[f32]) -> Vec<f32> {
        samples
            .chunks(3)
            .flat_map(|chunk| {
                if chunk.len() >= 2 {
                    vec![chunk[0], chunk[1]]
                } else {
                    chunk.to_vec()
                }
            })
            .collect()
    }

    fn process_frame(&mut self, frame: &[f32]) -> bool {
        let is_voice = match self.engine.compute(frame) {
            Ok(result) => {
                let voice = result.prob >= SPEECH_THRESHOLD;
                if crate::debug::transcription_debug_enabled() {
                    debug!(
                        probability = format!("{:.3}", result.prob),
                        speech = voice,
                        "VAD frame"
                    );
                }
                voice
            }
            Err(e) => {
                tracing::warn!("VAD compute error: {e}");
                true // Fail open
            }
        };

        match (self.in_speech, is_voice) {
            // Potential speech start — accumulate onset frames
            (false, true) => {
                self.onset_count += 1;
                if self.onset_count >= ONSET_FRAMES {
                    self.in_speech = true;
                    self.hangover_remaining = HANGOVER_FRAMES;
                    self.onset_count = 0;
                    true
                } else {
                    false
                }
            }
            // Ongoing speech
            (true, true) => {
                self.hangover_remaining = HANGOVER_FRAMES;
                true
            }
            // Speech ended — hangover
            (true, false) => {
                if self.hangover_remaining > 0 {
                    self.hangover_remaining -= 1;
                    true
                } else {
                    self.in_speech = false;
                    false
                }
            }
            // Silence
            (false, false) => {
                self.onset_count = 0;
                false
            }
        }
    }
}

impl AudioFilter for SileroVadFilter {
    fn kind(&self) -> AudioFilterKind {
        AudioFilterKind::SileroVad
    }

    fn process(&mut self, audio: &[f32]) -> bool {
        // Convert to 16kHz if needed
        let samples_16k: Vec<f32>;
        let input = if self.source_sample_rate == VAD_SAMPLE_RATE {
            audio
        } else if self.source_sample_rate == 24_000 {
            samples_16k = Self::decimate_24k_to_16k(audio);
            &samples_16k
        } else {
            audio
        };

        self.buffer.extend_from_slice(input);

        // Process complete Silero frames (480 samples = 30ms at 16kHz)
        let mut result = self.in_speech || self.hangover_remaining > 0;
        while self.buffer.len() >= VAD_FRAME_SAMPLES {
            let frame: Vec<f32> = self.buffer.drain(..VAD_FRAME_SAMPLES).collect();
            result = self.process_frame(&frame);
        }

        result
    }

    fn reset(&mut self) {
        self.buffer.clear();
        self.hangover_remaining = 0;
        self.onset_count = 0;
        self.in_speech = false;
    }
}
