use std::path::Path;
use std::sync::Mutex;

use candle_core::{Device, Tensor};
use tracing::info;

use super::{EngineError, TranscriptionEngine, TranscriptionSegment};

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
    fn to_lm_config(&self) -> moshi::lm::Config {
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
            // VAD extra heads: 4 heads, 6 dims each (0.5s, 1s, 2s, 3s horizons)
            extra_heads: Some(moshi::lm::ExtraHeadsConfig {
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
    /// Path to model directory, needed to reload mimi for state reset
    model_path: std::path::PathBuf,
}

/// Kyutai STT engine implementation.
/// Uses moshi crate for Mimi audio codec + decoder-only transformer.
/// Streaming: feed 1920-sample (80ms @ 24kHz) chunks, get words back.
pub struct KyutaiEngine {
    model: Mutex<Option<LoadedModel>>,
}

impl KyutaiEngine {
    pub fn new() -> Self {
        Self {
            model: Mutex::new(None),
        }
    }

    fn select_device() -> Result<Device, EngineError> {
        if candle_core::utils::metal_is_available() {
            Device::new_metal(0).map_err(|e| EngineError::LoadError(format!("Metal init: {e}")))
        } else {
            info!("Metal not available, falling back to CPU");
            Ok(Device::Cpu)
        }
    }

    /// Reset the ASR state for a new recording session.
    /// Recreates the moshi::asr::State to clear KV cache and internal buffers.
    pub fn reset_state(&self) -> Result<(), EngineError> {
        let mut guard = self.model.lock().map_err(|_| EngineError::InferenceError("Lock poisoned".into()))?;
        let model = guard.as_mut().ok_or(EngineError::NotInitialized)?;

        // Reload mimi and recreate state
        let mimi_path = model.model_path.join(&model.config.mimi_name);
        let audio_tokenizer = moshi::mimi::load(
            mimi_path.to_str().ok_or_else(|| EngineError::LoadError("Invalid mimi path".into()))?,
            Some(32),
            &model.device,
        )
        .map_err(|e| EngineError::LoadError(format!("Mimi reload: {e}")))?;

        let asr_delay_in_tokens = (model.config.stt_config.audio_delay_seconds * 12.5) as usize;

        // Recreate the LM model for fresh KV cache
        let dtype = model.device.bf16_default_to_f32();
        let model_file = model.model_path.join("model.safetensors");
        let vb_lm = unsafe {
            candle_nn::VarBuilder::from_mmaped_safetensors(&[&model_file], dtype, &model.device)
                .map_err(|e| EngineError::LoadError(format!("Model weights reload: {e}")))?
        };
        let lm = moshi::lm::LmModel::new(
            &model.config.to_lm_config(),
            moshi::nn::MaybeQuantizedVarBuilder::Real(vb_lm),
        )
        .map_err(|e| EngineError::LoadError(format!("LM model reload: {e}")))?;

        model.state = moshi::asr::State::new(1, asr_delay_in_tokens, 0., audio_tokenizer, lm)
            .map_err(|e| EngineError::LoadError(format!("ASR state reset: {e}")))?;

        eprintln!("[souffle] ASR state reset for new session");
        Ok(())
    }
}

impl TranscriptionEngine for KyutaiEngine {
    fn name(&self) -> &str {
        "Kyutai STT 1B (FR/EN)"
    }

    fn supported_languages(&self) -> Vec<String> {
        vec!["fr".into(), "en".into()]
    }

    fn supports_streaming(&self) -> bool {
        true
    }

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

        // Load main transformer model (safetensors)
        let model_file = model_path.join("model.safetensors");
        if !model_file.exists() {
            return Err(EngineError::ModelNotFound(model_file));
        }
        let dtype = device.bf16_default_to_f32();
        let vb_lm = unsafe {
            candle_nn::VarBuilder::from_mmaped_safetensors(&[&model_file], dtype, &device)
                .map_err(|e| EngineError::LoadError(format!("Model weights: {e}")))?
        };
        let lm = moshi::lm::LmModel::new(
            &config.to_lm_config(),
            moshi::nn::MaybeQuantizedVarBuilder::Real(vb_lm),
        )
        .map_err(|e| EngineError::LoadError(format!("LM model init: {e}")))?;
        info!("Transformer model loaded");

        // Load Mimi audio tokenizer
        let mimi_path = model_path.join(&config.mimi_name);
        let audio_tokenizer = moshi::mimi::load(
            mimi_path.to_str().ok_or_else(|| EngineError::LoadError("Invalid mimi path".into()))?,
            Some(32),
            &device,
        )
        .map_err(|e| EngineError::LoadError(format!("Mimi codec: {e}")))?;
        info!("Mimi audio codec loaded");

        // Create ASR state machine
        let asr_delay_in_tokens = (config.stt_config.audio_delay_seconds * 12.5) as usize;
        let state = moshi::asr::State::new(1, asr_delay_in_tokens, 0., audio_tokenizer, lm)
            .map_err(|e| EngineError::LoadError(format!("ASR state init: {e}")))?;

        info!("Kyutai STT model fully loaded");

        let mut guard = self.model.lock().map_err(|_| EngineError::LoadError("Lock poisoned".into()))?;
        *guard = Some(LoadedModel {
            state,
            text_tokenizer,
            config,
            device,
            model_path: model_path.to_path_buf(),
        });

        Ok(())
    }

    fn unload_model(&mut self) -> Result<(), EngineError> {
        let mut guard = self.model.lock().map_err(|_| EngineError::LoadError("Lock poisoned".into()))?;
        *guard = None;
        info!("Kyutai STT model unloaded");
        Ok(())
    }

    fn transcribe(
        &self,
        audio: &[f32],
        _language: Option<&str>,
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let mut guard = self.model.lock().map_err(|_| EngineError::InferenceError("Lock poisoned".into()))?;
        let model = guard.as_mut().ok_or(EngineError::NotInitialized)?;

        let mut segments = Vec::new();

        static FRAME_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        // Save first 3 seconds of audio to WAV for debugging
        static DEBUG_SAMPLES: std::sync::Mutex<Option<Vec<f32>>> = std::sync::Mutex::new(None);
        {
            let mut dbg = DEBUG_SAMPLES.lock().unwrap();
            if dbg.is_none() && FRAME_COUNT.load(std::sync::atomic::Ordering::Relaxed) == 0 {
                *dbg = Some(Vec::with_capacity(24_000 * 3));
            }
            if let Some(ref mut buf) = *dbg {
                if buf.len() < 24_000 * 3 {
                    buf.extend_from_slice(audio);
                } else if !buf.is_empty() {
                    // Save once and clear
                    let path = dirs_next::data_dir()
                        .unwrap_or_else(|| std::path::PathBuf::from("."))
                        .join("com.souffle.app")
                        .join("debug_engine_input.wav");
                    if let Ok(mut w) = hound::WavWriter::create(&path, hound::WavSpec {
                        channels: 1, sample_rate: 24_000,
                        bits_per_sample: 32, sample_format: hound::SampleFormat::Float,
                    }) {
                        for &s in buf.iter() { let _ = w.write_sample(s); }
                        let _ = w.finalize();
                        eprintln!("[souffle] DEBUG: Saved engine input audio to {}", path.display());
                    }
                    buf.clear();
                }
            }
        }

        // Log audio amplitude reaching the engine
        if !audio.is_empty() {
            let max_amp = audio.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
            let rms = (audio.iter().map(|s| s * s).sum::<f32>() / audio.len() as f32).sqrt();
            let frame_num = FRAME_COUNT.load(std::sync::atomic::Ordering::Relaxed);
            if frame_num < 5 || frame_num % 50 == 0 {
                eprintln!(
                    "[souffle] Engine input: {} samples, max_amp={max_amp:.4}, rms={rms:.6}",
                    audio.len()
                );
            }
        }

        // Process audio in 1920-sample frames (80ms at 24kHz)
        for chunk in audio.chunks(1920) {
            let padded;
            let chunk_data = if chunk.len() < 1920 {
                padded = {
                    let mut v = chunk.to_vec();
                    v.resize(1920, 0.0);
                    v
                };
                &padded[..]
            } else {
                chunk
            };

            let pcm_tensor = Tensor::new(chunk_data, &model.device)
                .and_then(|t| t.reshape((1, 1, 1920)))
                .map_err(|e| EngineError::InferenceError(format!("Tensor creation: {e}")))?;

            let asr_msgs = model
                .state
                .step_pcm(pcm_tensor, None, &().into(), |_, _, _| ())
                .map_err(|e| EngineError::InferenceError(format!("step_pcm: {e}")))?;

            let frame_num = FRAME_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            // Log message types for first 20 frames then every 50th
            if frame_num < 20 || frame_num % 50 == 0 {
                let mut words = 0;
                let mut end_words = 0;
                let mut steps = 0;
                for msg in &asr_msgs {
                    match msg {
                        moshi::asr::AsrMsg::Word { .. } => words += 1,
                        moshi::asr::AsrMsg::EndWord { .. } => end_words += 1,
                        moshi::asr::AsrMsg::Step { prs, .. } => {
                            steps += 1;
                            if frame_num < 10 || frame_num % 50 == 0 {
                                // Show VAD probabilities
                                let vad_str: Vec<String> = prs.iter()
                                    .map(|p| format!("{:.2}", p[0]))
                                    .collect();
                                eprintln!(
                                    "[souffle] Frame {frame_num}: Step VAD=[{}]",
                                    vad_str.join(", ")
                                );
                            }
                        }
                    }
                }
                if words > 0 || end_words > 0 {
                    eprintln!(
                        "[souffle] Frame {frame_num}: {words} words, {end_words} end_words, {steps} steps"
                    );
                }
            }

            for msg in &asr_msgs {
                match msg {
                    moshi::asr::AsrMsg::Word { tokens, start_time, .. } => {
                        let text = model
                            .text_tokenizer
                            .decode_piece_ids(tokens)
                            .unwrap_or_default();
                        eprintln!("[souffle] WORD: tokens={tokens:?} text={text:?} t={start_time:.2}");
                        if !text.is_empty() {
                            // Emit immediately — the Word message contains the fully
                            // decoded text. EndWord is just a timing boundary that can
                            // arrive 5+ seconds later; waiting for it causes truncation
                            // at end of speech.
                            segments.push(TranscriptionSegment {
                                text,
                                start_time: *start_time,
                                end_time: *start_time,
                                is_final: true,
                                language: None,
                                confidence: None,
                            });
                        }
                    }
                    moshi::asr::AsrMsg::EndWord { .. } => {
                        // Timing boundary only — word was already emitted on Word event
                    }
                    moshi::asr::AsrMsg::Step { .. } => {
                        // VAD probabilities — logged above
                    }
                }
            }
        }

        Ok(segments)
    }

    fn flush(&self) -> Result<Vec<TranscriptionSegment>, EngineError> {
        // Feed silence suffix to push any remaining words out of the model's
        // internal pipeline (audio_delay + 1 second of silence)
        let guard = self.model.lock().map_err(|_| EngineError::InferenceError("Lock poisoned".into()))?;
        let model = guard.as_ref().ok_or(EngineError::NotInitialized)?;
        let suffix_seconds = model.config.stt_config.audio_delay_seconds + 1.0;
        let silence_samples = (suffix_seconds * 24_000.0) as usize;
        let silence = vec![0.0f32; silence_samples];
        drop(guard);

        self.transcribe(&silence, None)
    }

    fn memory_usage(&self) -> Option<u64> {
        // Approximate: 1B params at f16 ≈ 2GB + Mimi ≈ 200MB + KV cache
        Some(4_000_000_000)
    }
}
