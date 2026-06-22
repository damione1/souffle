use std::fs::File;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use candle_core::{Device, Tensor};
use tracing::{debug, info, trace};

use super::{
    AudioInputRequirements, EngineError, TranscriptionEngine, TranscriptionSegment,
    collapse_whitespace,
};
use crate::constants::{MIMI_FRAME_SIZE, MIMI_FRAMES_PER_SECOND, SAMPLE_RATE};
use crate::platform::with_autorelease_pool;

/// Extra-head index used for pause detection, matching Kyutai's reference
/// stt-rs example (`prs[2][0] > 0.5`).
const VAD_PAUSE_HEAD: usize = 2;
const VAD_PAUSE_THRESHOLD: f32 = 0.5;
/// Safety margin (frames) on top of the ASR delay before trusting the VAD
/// pause streak: semantic VAD can fire slightly before speech fully clears.
const VAD_FLUSH_MARGIN_FRAMES: usize = 6;

/// Debug frame counter — reset per session for clean logging
static FRAME_COUNT: AtomicU64 = AtomicU64::new(0);
/// Debug audio buffer — captures first 3s of each session for offline analysis
static DEBUG_SAMPLES: Mutex<Option<Vec<f32>>> = Mutex::new(None);

/// Kyutai STT model configuration, deserialized from config.json
#[derive(Debug, serde::Deserialize)]
pub struct SttConfig {
    pub audio_silence_prefix_seconds: f64,
    pub audio_delay_seconds: f64,
}

#[derive(Debug, serde::Deserialize)]
pub struct KyutaiConfig {
    pub mimi_name: String,
    pub tokenizer_name: String,
    pub card: usize,
    pub text_card: usize,
    pub dim: usize,
    pub n_q: usize,
    pub context: usize,
    pub max_period: f64,
    pub num_heads: usize,
    pub num_layers: usize,
    pub causal: bool,
    pub stt_config: SttConfig,
}

impl KyutaiConfig {
    fn to_lm_config(&self, has_extra_heads: bool) -> moshi::lm::Config {
        let transformer = moshi::transformer::Config {
            d_model: self.dim,
            num_heads: self.num_heads,
            num_layers: self.num_layers,
            dim_feedforward: self.dim * 4,
            causal: self.causal,
            norm_first: true,
            bias_ff: false,
            bias_attn: false,
            layer_scale: None,
            context: self.context,
            max_period: self.max_period as usize,
            use_conv_block: false,
            use_conv_bias: true,
            cross_attention: None,
            gating: Some(candle_nn::Activation::Silu),
            norm: moshi::NormType::RmsNorm,
            positional_embedding: moshi::transformer::PositionalEmbedding::Rope,
            conv_layout: false,
            conv_kernel_size: 3,
            kv_repeat: 1,
            max_seq_len: 4096 * 4,
            shared_cross_attn: false,
        };
        moshi::lm::Config {
            transformer,
            depformer: None,
            audio_vocab_size: self.card + 1,
            text_in_vocab_size: self.text_card + 1,
            text_out_vocab_size: self.text_card,
            audio_codebooks: self.n_q,
            conditioners: Default::default(),
            extra_heads: has_extra_heads.then_some(moshi::lm::ExtraHeadsConfig {
                num_heads: 4,
                dim: 6,
            }),
        }
    }
}

/// Loaded model components — kept together so they can be used by the inference loop
struct LoadedModel {
    state: moshi::asr::State,
    text_tokenizer: sentencepiece::SentencePieceProcessor,
    config: KyutaiConfig,
    device: Device,
    #[allow(dead_code)]
    model_path: std::path::PathBuf,
    /// Silence prefix (config audio_silence_prefix_seconds) still to be fed
    /// before the first real audio of the session.
    prefix_pending: bool,
    /// Word timestamps include the silence prefix; subtract this to report
    /// times relative to the real audio.
    time_offset_seconds: f64,
    /// Consecutive frames where the semantic VAD pause head fired.
    vad_pause_streak: usize,
}

/// Kyutai STT engine implementation.
/// Uses moshi crate for Mimi audio codec + decoder-only transformer.
/// Streaming: feed 1920-sample (80ms @ 24kHz) chunks, get words back.
pub struct KyutaiEngine {
    model: Option<LoadedModel>,
}

impl Default for KyutaiEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl KyutaiEngine {
    pub fn new() -> Self {
        Self { model: None }
    }

    fn select_device() -> Result<Device, EngineError> {
        if candle_core::utils::metal_is_available() {
            Device::new_metal(0).map_err(|e| EngineError::LoadError(format!("Metal init: {e}")))
        } else {
            info!("Metal not available, falling back to CPU");
            Ok(Device::Cpu)
        }
    }

    fn build_state(
        device: &Device,
        model_path: &Path,
        config: &KyutaiConfig,
    ) -> Result<moshi::asr::State, EngineError> {
        let mimi_path = model_path.join(&config.mimi_name);
        let audio_tokenizer = moshi::mimi::load(
            mimi_path
                .to_str()
                .ok_or_else(|| EngineError::LoadError("Invalid mimi path".into()))?,
            Some(32),
            device,
        )
        .map_err(|e| EngineError::LoadError(format!("Mimi reload: {e}")))?;

        let dtype = device.bf16_default_to_f32();
        let model_file = model_path.join("model.safetensors");
        let has_extra_heads = Self::detect_extra_heads(&model_file)?;
        let vb_lm = unsafe {
            candle_nn::VarBuilder::from_mmaped_safetensors(&[&model_file], dtype, device)
                .map_err(|e| EngineError::LoadError(format!("Model weights reload: {e}")))?
        };
        let lm = moshi::lm::LmModel::new(
            &config.to_lm_config(has_extra_heads),
            moshi::nn::MaybeQuantizedVarBuilder::Real(vb_lm),
        )
        .map_err(|e| EngineError::LoadError(format!("LM model reload: {e}")))?;

        let asr_delay_in_tokens =
            (config.stt_config.audio_delay_seconds * MIMI_FRAMES_PER_SECOND) as usize;
        moshi::asr::State::new(1, asr_delay_in_tokens, 0., audio_tokenizer, lm)
            .map_err(|e| EngineError::LoadError(format!("ASR state init: {e}")))
    }

    fn build_loaded_model(
        device: Device,
        model_path: std::path::PathBuf,
        config: KyutaiConfig,
        text_tokenizer: sentencepiece::SentencePieceProcessor,
    ) -> Result<LoadedModel, EngineError> {
        let state = Self::build_state(&device, &model_path, &config)?;
        Ok(LoadedModel {
            state,
            text_tokenizer,
            config,
            device,
            model_path,
            prefix_pending: true,
            time_offset_seconds: 0.0,
            vad_pause_streak: 0,
        })
    }

    /// Silence prefix length in whole Mimi frames. Rounded up so the prefix
    /// never leaves a partial frame that would zero-pad real audio mid-stream.
    fn prefix_frame_count(prefix_seconds: f64) -> usize {
        (prefix_seconds * MIMI_FRAMES_PER_SECOND).ceil() as usize
    }

    fn detect_extra_heads(model_file: &Path) -> Result<bool, EngineError> {
        let file = File::open(model_file)
            .map_err(|e| EngineError::LoadError(format!("Weights open failed: {e}")))?;
        let mmap = unsafe { memmap2::Mmap::map(&file) }
            .map_err(|e| EngineError::LoadError(format!("Weights mmap failed: {e}")))?;
        let (_, metadata) = safetensors::tensor::SafeTensors::read_metadata(&mmap)
            .map_err(|e| EngineError::LoadError(format!("Weights metadata read failed: {e}")))?;
        Ok(metadata.info("extra_heads.0.weight").is_some())
    }

    fn synchronize_device(device: &Device, context: &str) -> Result<(), EngineError> {
        device
            .synchronize()
            .map_err(|e| EngineError::InferenceError(format!("{context}: {e}")))
    }

    /// Reset the ASR state for a new recording session.
    /// Full rebuild of Mimi + LM + State from disk because moshi's
    /// State::reset() does NOT reset model_step_idx, causing RoPE
    /// positional encoding to start at the wrong offset with empty KV caches.
    /// Teardown and rebuild use separate autorelease pools so stale Metal
    /// objects are drained before a fresh device/model is created.
    pub fn reset_state(&mut self) -> Result<(), EngineError> {
        FRAME_COUNT.store(0, Ordering::Relaxed);
        if let Ok(mut dbg) = DEBUG_SAMPLES.lock() {
            *dbg = None;
        }

        {
            let loaded = self.model.as_ref().ok_or(EngineError::NotInitialized)?;
            Self::synchronize_device(&loaded.device, "Metal sync before reset")?;
        }

        let old = self.model.take().ok_or(EngineError::NotInitialized)?;
        let LoadedModel {
            state: old_state,
            text_tokenizer,
            config,
            device: old_device,
            model_path,
            ..
        } = old;

        with_autorelease_pool(move || {
            drop(old_state);
            drop(old_device);
        });

        let rebuilt = with_autorelease_pool(move || -> Result<LoadedModel, EngineError> {
            let device = Self::select_device()?;
            Self::build_loaded_model(device, model_path, config, text_tokenizer)
        })?;

        self.model = Some(rebuilt);
        info!("ASR state rebuilt for new session");
        Ok(())
    }
}

impl TranscriptionEngine for KyutaiEngine {
    fn load_model(&mut self, model_path: &Path) -> Result<(), EngineError> {
        let device = Self::select_device()?;
        info!(device = ?device, "Loading Kyutai STT model");

        // Read config.json
        let config_path = model_path.join("config.json");
        let config_str = std::fs::read_to_string(&config_path)
            .map_err(|_| EngineError::ModelNotFound(config_path.clone()))?;
        let config: KyutaiConfig = serde_json::from_str(&config_str)
            .map_err(|e| EngineError::LoadError(format!("Invalid config.json: {e}")))?;

        // Load SentencePiece tokenizer
        let tokenizer_path = model_path.join(&config.tokenizer_name);
        let text_tokenizer = sentencepiece::SentencePieceProcessor::open(&tokenizer_path)
            .map_err(|e| EngineError::LoadError(format!("Tokenizer load failed: {e}")))?;
        info!("Tokenizer loaded");

        let model_file = model_path.join("model.safetensors");
        if !model_file.exists() {
            return Err(EngineError::ModelNotFound(model_file));
        }

        let loaded = with_autorelease_pool(move || {
            Self::build_loaded_model(device, model_path.to_path_buf(), config, text_tokenizer)
        })?;

        info!("Kyutai STT model fully loaded");

        self.model = Some(loaded);

        Ok(())
    }

    fn unload_model(&mut self) -> Result<(), EngineError> {
        if let Some(loaded) = self.model.as_ref() {
            Self::synchronize_device(&loaded.device, "Metal sync before unload")?;
        }
        if let Some(loaded) = self.model.take() {
            with_autorelease_pool(move || {
                drop(loaded);
            });
        }
        info!("Kyutai STT model unloaded");
        Ok(())
    }

    fn transcribe(
        &mut self,
        audio: &[f32],
        _language: Option<&str>,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let debug_enabled = crate::debug::transcription_debug_enabled();
        let model = self.model.as_mut().ok_or(EngineError::NotInitialized)?;

        let mut segments = Vec::new();

        // Debug: save first 3s of audio per session to WAV for offline analysis
        if debug_enabled {
            let Ok(mut dbg) = DEBUG_SAMPLES.lock() else {
                return Ok(segments);
            };
            if dbg.is_none() && FRAME_COUNT.load(Ordering::Relaxed) == 0 {
                *dbg = Some(Vec::with_capacity(SAMPLE_RATE as usize * 3));
            }
            if let Some(ref mut buf) = *dbg {
                if buf.len() < SAMPLE_RATE as usize * 3 {
                    buf.extend_from_slice(audio);
                } else if !buf.is_empty() {
                    let path = crate::constants::app_data_dir().join("debug_engine_input.wav");
                    if let Ok(mut w) = hound::WavWriter::create(
                        &path,
                        hound::WavSpec {
                            channels: 1,
                            sample_rate: SAMPLE_RATE,
                            bits_per_sample: 32,
                            sample_format: hound::SampleFormat::Float,
                        },
                    ) {
                        for &s in buf.iter() {
                            let _ = w.write_sample(s);
                        }
                        let _ = w.finalize();
                        debug!(path = %path.display(), "Saved engine input audio");
                    }
                    buf.clear();
                }
            }
        }

        // Log audio amplitude reaching the engine
        if debug_enabled && !audio.is_empty() {
            let max_amp = audio.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
            let rms = (audio.iter().map(|s| s * s).sum::<f32>() / audio.len() as f32).sqrt();
            let frame_num = FRAME_COUNT.load(Ordering::Relaxed);
            if frame_num < 5 || frame_num.is_multiple_of(50) {
                debug!(
                    samples = audio.len(),
                    max_amp = format!("{max_amp:.4}"),
                    rms = format!("{rms:.6}"),
                    "Engine input"
                );
            }
        }

        // Clone device handle (cheap Arc clone) so closure can use it
        // without conflicting with mutable borrow of model.state
        let device = model.device.clone();

        // The model is trained with audio_silence_prefix_seconds of leading
        // silence; feed it before the first real audio of each session.
        let prefixed;
        let audio = if model.prefix_pending {
            model.prefix_pending = false;
            let prefix_frames =
                Self::prefix_frame_count(model.config.stt_config.audio_silence_prefix_seconds);
            if prefix_frames > 0 {
                model.time_offset_seconds = prefix_frames as f64 / MIMI_FRAMES_PER_SECOND;
                info!(
                    frames = prefix_frames,
                    seconds = model.time_offset_seconds,
                    "Feeding silence prefix before session audio"
                );
                prefixed = {
                    let mut v = vec![0.0f32; prefix_frames * MIMI_FRAME_SIZE];
                    v.extend_from_slice(audio);
                    v
                };
                &prefixed[..]
            } else {
                audio
            }
        } else {
            audio
        };

        // Process audio in MIMI_FRAME_SIZE-sample frames (80ms at 24kHz)
        for chunk in audio.chunks(MIMI_FRAME_SIZE) {
            let padded;
            let chunk_data = if chunk.len() < MIMI_FRAME_SIZE {
                padded = {
                    let mut v = chunk.to_vec();
                    v.resize(MIMI_FRAME_SIZE, 0.0);
                    v
                };
                &padded[..]
            } else {
                chunk
            };

            // Wrap Metal operations in autorelease pool to drain ObjC objects
            // created by candle's Metal backend (matmul, attention, etc.).
            // Without this, autoreleased objects accumulate and corrupt GPU
            // memory after ~3 recording sessions.
            let asr_msgs = with_autorelease_pool(|| {
                let pcm_tensor = Tensor::new(chunk_data, &device)
                    .and_then(|t| t.reshape((1, 1, MIMI_FRAME_SIZE)))
                    .map_err(|e| EngineError::InferenceError(format!("Tensor creation: {e}")))?;

                model
                    .state
                    .step_pcm(
                        pcm_tensor,
                        None,
                        &().into(),
                        |items, text_tensor, _audio_tensors| {
                            // Debug: log what the model is producing
                            let frame = FRAME_COUNT.load(Ordering::Relaxed);
                            if debug_enabled
                                && (frame < 20 || frame.is_multiple_of(50))
                                && let Ok(text_vals) = text_tensor.to_vec2::<u32>()
                            {
                                for (i, item) in items.iter().enumerate() {
                                    let tv = text_vals
                                        .get(i)
                                        .map(|v| format!("{v:?}"))
                                        .unwrap_or_default();
                                    trace!(
                                        frame,
                                        batch = i,
                                        text_token = item.text_token(),
                                        first_step = item.is_first_step(),
                                        input_text = tv,
                                        "pre-forward"
                                    );
                                }
                            }
                        },
                    )
                    .map_err(|e| EngineError::InferenceError(format!("step_pcm: {e}")))
            })?;

            let frame_num = FRAME_COUNT.fetch_add(1, Ordering::Relaxed);

            // Log message types for first 20 frames then every 50th
            if debug_enabled && (frame_num < 20 || frame_num.is_multiple_of(50)) {
                let mut words = 0;
                let mut end_words = 0;
                let mut steps = 0;
                for msg in &asr_msgs {
                    match msg {
                        moshi::asr::AsrMsg::Word { .. } => words += 1,
                        moshi::asr::AsrMsg::EndWord { .. } => end_words += 1,
                        moshi::asr::AsrMsg::Step { step_idx, prs, .. } => {
                            steps += 1;
                            if frame_num < 10 || frame_num.is_multiple_of(50) {
                                let vad_str: Vec<String> =
                                    prs.iter().map(|p| format!("{:.2}", p[0])).collect();
                                trace!(
                                    frame = frame_num,
                                    model_step = step_idx,
                                    vad = vad_str.join(", "),
                                    "Step VAD"
                                );
                            }
                        }
                    }
                }
                if words > 0 || end_words > 0 {
                    debug!(frame = frame_num, words, end_words, steps, "ASR messages");
                }
            }

            for msg in &asr_msgs {
                match msg {
                    moshi::asr::AsrMsg::Word {
                        tokens, start_time, ..
                    } => {
                        let text = model
                            .text_tokenizer
                            .decode_piece_ids(tokens)
                            .unwrap_or_default();
                        if debug_enabled {
                            debug!(tokens = ?tokens, text = ?text, t = format!("{start_time:.2}"), "WORD emitted");
                        }
                        if !text.is_empty() {
                            // Emit immediately — the Word message contains the fully
                            // decoded text. EndWord is just a timing boundary that can
                            // arrive 5+ seconds later; waiting for it causes truncation
                            // at end of speech.
                            let start_time = (*start_time - model.time_offset_seconds).max(0.0);
                            segments.push(TranscriptionSegment {
                                text,
                                start_time,
                                end_time: start_time,
                                is_final: true,
                                language: None,
                                confidence: None,
                                speaker: None,
                            });
                        }
                    }
                    moshi::asr::AsrMsg::EndWord { .. } => {
                        // Timing boundary only — word was already emitted on Word event
                    }
                    moshi::asr::AsrMsg::Step { prs, .. } => {
                        // Semantic VAD (models with extra heads only): track how
                        // long the pause head has been firing so flush() can skip
                        // the silence suffix when the delay window is already drained.
                        if let Some(p) = prs.get(VAD_PAUSE_HEAD).and_then(|h| h.first()) {
                            if *p > VAD_PAUSE_THRESHOLD {
                                model.vad_pause_streak += 1;
                            } else {
                                model.vad_pause_streak = 0;
                            }
                        }
                    }
                }
            }
        }

        Ok(segments)
    }

    fn flush(&mut self) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let model = self.model.as_ref().ok_or(EngineError::NotInitialized)?;

        // Words are emitted audio_delay after they are spoken. If the semantic
        // VAD has reported a pause for longer than that delay (plus margin),
        // every word has already cleared the pipeline and the silence suffix
        // would only burn inference time at stop.
        let delay_frames =
            (model.config.stt_config.audio_delay_seconds * MIMI_FRAMES_PER_SECOND) as usize;
        if model.vad_pause_streak >= delay_frames + VAD_FLUSH_MARGIN_FRAMES {
            info!(
                streak = model.vad_pause_streak,
                delay_frames, "VAD pause covers ASR delay, skipping silence flush"
            );
            return Ok(Vec::new());
        }

        // Feed silence suffix to push any remaining words out of the model's
        // internal pipeline (audio_delay + 1 second of silence)
        let suffix_seconds = model.config.stt_config.audio_delay_seconds + 1.0;
        let silence_samples = (suffix_seconds * SAMPLE_RATE as f64) as usize;
        let silence = vec![0.0f32; silence_samples];

        self.transcribe(&silence, None)
    }

    fn reset_state(&mut self) -> Result<(), EngineError> {
        KyutaiEngine::reset_state(self)
    }

    fn audio_requirements(&self) -> AudioInputRequirements {
        AudioInputRequirements {
            sample_rate_hz: SAMPLE_RATE,
            channels: 1,
            chunk_size_samples: MIMI_FRAME_SIZE as u32,
        }
    }

    fn mic_gain(&self) -> f32 {
        1.0
    }

    fn normalize_text(&self, text: &str) -> String {
        // SentencePiece uses ▁ (U+2581) as word-boundary marker.
        // Replace with space, then trim/collapse.
        let normalized = text.replace('▁', " ");
        collapse_whitespace(&normalized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_frame_count_zero_for_no_prefix() {
        // stt-1b-en_fr-candle config: audio_silence_prefix_seconds = 0.0
        assert_eq!(KyutaiEngine::prefix_frame_count(0.0), 0);
    }

    #[test]
    fn prefix_frame_count_rounds_up_to_whole_frames() {
        // stt-2.6b-en-candle config: audio_silence_prefix_seconds = 1.0
        // 1.0s * 12.5 = 12.5 frames -> 13 whole frames, never a partial
        // frame that would zero-pad real audio mid-stream.
        assert_eq!(KyutaiEngine::prefix_frame_count(1.0), 13);
        assert_eq!(KyutaiEngine::prefix_frame_count(0.5), 7);
        assert_eq!(KyutaiEngine::prefix_frame_count(2.0), 25);
    }
}
