use std::fs::File;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use candle_core::{Device, Tensor};
use tracing::{debug, info, trace};

use super::{
    AudioInputRequirements, EngineError, Speaker, TranscriptionEngine, TranscriptionSegment,
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
/// Semantic-pause streak before a soft context refresh is allowed. ~0.5s at
/// 12.5 Hz — long enough to sit between utterances, short enough to fire
/// before the LM context window saturates.
const REFRESH_PAUSE_FRAMES: usize = 6;
/// Soft refresh fires at this fraction of `config.context` when pausing
/// (Kyutai/Unmute recommend clearing KV between speech turns).
const REFRESH_SOFT_CONTEXT_NUM: usize = 6;
const REFRESH_SOFT_CONTEXT_DEN: usize = 10;
/// Hard deadline margin: force refresh this many frames before `context`
/// even mid-speech so attention never runs fully masked.
const REFRESH_HARD_MARGIN_FRAMES: usize = 25;

/// Extract frame `f` (MIMI_FRAME_SIZE samples) from `buf`, zero-padding when the
/// buffer is short or the frame is past its end. Used to align the two diarized
/// lanes into equal-length batched steps.
fn frame_at(buf: &[f32], f: usize) -> Vec<f32> {
    let start = f * MIMI_FRAME_SIZE;
    let mut frame = vec![0.0f32; MIMI_FRAME_SIZE];
    if start < buf.len() {
        let end = (start + MIMI_FRAME_SIZE).min(buf.len());
        frame[..end - start].copy_from_slice(&buf[start..end]);
    }
    frame
}

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

/// Whether a proactive KV-cache refresh should run, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshKind {
    /// Pause-aligned refresh inside the soft context window (preferred).
    SoftPause,
    /// Forced refresh before the LM context window saturates.
    HardDeadline,
}

/// Decide whether to clear the ASR KV cache before the next frame.
///
/// Kyutai STT is a decoder-only model with a finite `context` (375 frames /
/// ~30s on stt-2.6b). Past that window inference still runs but Word emission
/// can starve — matching the production "frames climb, segments flat" freeze.
/// Upstream guidance (Unmute #168) is to reset between speech turns; we do the
/// local equivalent with `State::reset()` rather than a full Metal rebuild.
fn should_refresh(
    frames_since_refresh: usize,
    context: usize,
    pausing: bool,
) -> Option<RefreshKind> {
    if context == 0 || frames_since_refresh == 0 {
        return None;
    }
    let soft = (context * REFRESH_SOFT_CONTEXT_NUM) / REFRESH_SOFT_CONTEXT_DEN;
    let hard = context.saturating_sub(REFRESH_HARD_MARGIN_FRAMES);
    if frames_since_refresh >= hard {
        Some(RefreshKind::HardDeadline)
    } else if pausing && frames_since_refresh >= soft {
        Some(RefreshKind::SoftPause)
    } else {
        None
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
    /// before the first real audio of the current refresh epoch.
    prefix_pending: bool,
    /// Prefix duration for the current epoch; subtracted from moshi times.
    time_offset_seconds: f64,
    /// Wall-clock seconds of real audio attributed to prior refresh epochs,
    /// so Word timestamps stay monotone across soft KV clears.
    epoch_origin_seconds: f64,
    /// LM frames since the last soft/hard refresh (includes this epoch's prefix).
    frames_since_refresh: usize,
    /// Soft context refreshes performed this session (diagnostics).
    refresh_count: u64,
    /// Consecutive frames where the semantic VAD pause head fired.
    vad_pause_streak: usize,
}

/// Kyutai STT engine implementation.
/// Uses moshi crate for Mimi audio codec + decoder-only transformer.
/// Streaming: feed 1920-sample (80ms @ 24kHz) chunks, get words back.
pub struct KyutaiEngine {
    model: Option<LoadedModel>,
    /// When true, the streaming state is built with batch size 2 so the mic (Me)
    /// and system audio (Them) legs are transcribed as independent batch lanes
    /// of one model. Takes effect on the next `reset_state`.
    diarize: bool,
}

impl Default for KyutaiEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl KyutaiEngine {
    pub fn new() -> Self {
        Self {
            model: None,
            diarize: false,
        }
    }

    /// moshi batch size for the current mode: 2 lanes when diarizing, else 1.
    fn batch_size(&self) -> usize {
        if self.diarize { 2 } else { 1 }
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
        batch_size: usize,
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
        moshi::asr::State::new(batch_size, asr_delay_in_tokens, 0., audio_tokenizer, lm)
            .map_err(|e| EngineError::LoadError(format!("ASR state init: {e}")))
    }

    fn build_loaded_model(
        device: Device,
        model_path: std::path::PathBuf,
        config: KyutaiConfig,
        text_tokenizer: sentencepiece::SentencePieceProcessor,
        batch_size: usize,
    ) -> Result<LoadedModel, EngineError> {
        let state = Self::build_state(&device, &model_path, &config, batch_size)?;
        Ok(LoadedModel {
            state,
            text_tokenizer,
            config,
            device,
            model_path,
            prefix_pending: true,
            time_offset_seconds: 0.0,
            epoch_origin_seconds: 0.0,
            frames_since_refresh: 0,
            refresh_count: 0,
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

    /// Map a moshi word timestamp into session wall-clock seconds, accounting
    /// for the current epoch's silence prefix and prior soft-refresh epochs.
    fn word_start_time(model: &LoadedModel, moshi_start: f64) -> f64 {
        Self::word_start_time_raw(
            moshi_start,
            model.time_offset_seconds,
            model.epoch_origin_seconds,
        )
    }

    fn word_start_time_raw(
        moshi_start: f64,
        time_offset_seconds: f64,
        epoch_origin_seconds: f64,
    ) -> f64 {
        (moshi_start - time_offset_seconds + epoch_origin_seconds).max(0.0)
    }

    /// Soft KV-cache clear: empties LM/Mimi/ItemState without rebuilding Metal
    /// devices or remapping weights. Preferred over full `reset_state` mid-session.
    fn refresh_loaded(model: &mut LoadedModel, kind: RefreshKind) -> Result<(), EngineError> {
        let frames = model.frames_since_refresh;
        let context = model.config.context;
        let real_secs =
            frames as f64 / MIMI_FRAMES_PER_SECOND - model.time_offset_seconds;
        model.epoch_origin_seconds += real_secs.max(0.0);
        model
            .state
            .reset()
            .map_err(|e| EngineError::InferenceError(format!("ASR context refresh: {e}")))?;
        model.prefix_pending = true;
        model.time_offset_seconds = 0.0;
        model.frames_since_refresh = 0;
        model.vad_pause_streak = 0;
        model.refresh_count = model.refresh_count.saturating_add(1);
        info!(
            kind = ?kind,
            frames_before_refresh = frames,
            context,
            refresh_count = model.refresh_count,
            epoch_origin_seconds = format!("{:.2}", model.epoch_origin_seconds),
            "ASR context refreshed (soft KV clear)"
        );
        Ok(())
    }

    fn maybe_refresh_before_frame(model: &mut LoadedModel) -> Result<(), EngineError> {
        let pausing = model.vad_pause_streak >= REFRESH_PAUSE_FRAMES;
        if let Some(kind) =
            should_refresh(model.frames_since_refresh, model.config.context, pausing)
        {
            Self::refresh_loaded(model, kind)?;
        }
        Ok(())
    }

    fn note_vad_pause(model: &mut LoadedModel, prs: &[Vec<f32>]) {
        if let Some(p) = prs.get(VAD_PAUSE_HEAD).and_then(|h| h.first()) {
            if *p > VAD_PAUSE_THRESHOLD {
                model.vad_pause_streak += 1;
            } else {
                model.vad_pause_streak = 0;
            }
        }
    }

    /// Feed the configured silence prefix as real LM frames (counts toward
    /// the context budget) and set `time_offset_seconds` for this epoch.
    fn feed_silence_prefix(
        model: &mut LoadedModel,
        device: &Device,
        debug_enabled: bool,
        segments: &mut Vec<TranscriptionSegment>,
    ) -> Result<(), EngineError> {
        model.prefix_pending = false;
        let prefix_frames =
            Self::prefix_frame_count(model.config.stt_config.audio_silence_prefix_seconds);
        if prefix_frames == 0 {
            model.time_offset_seconds = 0.0;
            return Ok(());
        }
        model.time_offset_seconds = prefix_frames as f64 / MIMI_FRAMES_PER_SECOND;
        info!(
            frames = prefix_frames,
            seconds = model.time_offset_seconds,
            "Feeding silence prefix before epoch audio"
        );
        let silence = vec![0.0f32; MIMI_FRAME_SIZE];
        for _ in 0..prefix_frames {
            let asr_msgs = if model.state.batch_size() == 2 {
                let mut data = Vec::with_capacity(2 * MIMI_FRAME_SIZE);
                data.extend_from_slice(&silence);
                data.extend_from_slice(&silence);
                Self::step_pcm_dual(model, device, &data)?
            } else {
                Self::step_pcm_single(model, device, &silence, debug_enabled)?
            };
            // Prefix frames can still emit delayed words from the previous
            // epoch's lookahead — keep consuming them with correct timestamps.
            Self::consume_asr_msgs(model, &asr_msgs, debug_enabled, segments);
        }
        Ok(())
    }

    fn step_pcm_single(
        model: &mut LoadedModel,
        device: &Device,
        chunk_data: &[f32],
        debug_enabled: bool,
    ) -> Result<Vec<moshi::asr::AsrMsg>, EngineError> {
        // Wrap Metal operations in autorelease pool to drain ObjC objects
        // created by candle's Metal backend (matmul, attention, etc.).
        // Without this, autoreleased objects accumulate and corrupt GPU
        // memory after ~3 recording sessions.
        let asr_msgs = with_autorelease_pool(|| {
            let pcm_tensor = Tensor::new(chunk_data, device)
                .and_then(|t| t.reshape((1, 1, MIMI_FRAME_SIZE)))
                .map_err(|e| EngineError::InferenceError(format!("Tensor creation: {e}")))?;

            model
                .state
                .step_pcm(
                    pcm_tensor,
                    None,
                    &().into(),
                    |items, text_tensor, _audio_tensors| {
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
        model.frames_since_refresh = model.frames_since_refresh.saturating_add(1);
        FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
        Ok(asr_msgs)
    }

    fn step_pcm_dual(
        model: &mut LoadedModel,
        device: &Device,
        data: &[f32],
    ) -> Result<Vec<moshi::asr::AsrMsg>, EngineError> {
        let asr_msgs = with_autorelease_pool(|| {
            let pcm_tensor = Tensor::new(data, device)
                .and_then(|t| t.reshape((2, 1, MIMI_FRAME_SIZE)))
                .map_err(|e| EngineError::InferenceError(format!("Tensor creation: {e}")))?;
            model
                .state
                .step_pcm(pcm_tensor, None, &().into(), |_, _, _| {})
                .map_err(|e| EngineError::InferenceError(format!("step_pcm: {e}")))
        })?;
        model.frames_since_refresh = model.frames_since_refresh.saturating_add(1);
        FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
        Ok(asr_msgs)
    }

    fn consume_asr_msgs(
        model: &mut LoadedModel,
        asr_msgs: &[moshi::asr::AsrMsg],
        debug_enabled: bool,
        segments: &mut Vec<TranscriptionSegment>,
    ) {
        let frame_num = FRAME_COUNT.load(Ordering::Relaxed).saturating_sub(1);
        let diarized = model.state.batch_size() == 2;

        if debug_enabled && (frame_num < 20 || frame_num.is_multiple_of(50)) {
            let mut words = 0;
            let mut end_words = 0;
            let mut steps = 0;
            for msg in asr_msgs {
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

        for msg in asr_msgs {
            match msg {
                moshi::asr::AsrMsg::Word {
                    tokens,
                    start_time,
                    batch_idx,
                } => {
                    let text = model
                        .text_tokenizer
                        .decode_piece_ids(tokens)
                        .unwrap_or_default();
                    if debug_enabled {
                        debug!(tokens = ?tokens, text = ?text, t = format!("{start_time:.2}"), "WORD emitted");
                    }
                    if text.is_empty() {
                        continue;
                    }
                    let start_time = Self::word_start_time(model, *start_time);
                    let speaker = if diarized {
                        Some(if *batch_idx == 0 {
                            Speaker::Me
                        } else {
                            Speaker::Them
                        })
                    } else {
                        None
                    };
                    segments.push(TranscriptionSegment {
                        text,
                        start_time,
                        end_time: start_time,
                        is_final: true,
                        language: None,
                        confidence: None,
                        speaker,
                    });
                }
                moshi::asr::AsrMsg::EndWord { .. } => {}
                moshi::asr::AsrMsg::Step { prs, .. } => {
                    Self::note_vad_pause(model, prs);
                }
            }
        }
    }

    fn context_window_stats(&self) -> Option<super::ContextWindowStats> {
        self.model.as_ref().map(|m| super::ContextWindowStats {
            context_frames: m.config.context,
            frames_since_refresh: m.frames_since_refresh,
            refresh_count: m.refresh_count,
        })
    }

    /// Reset the ASR state for a new recording session.
    /// Full rebuild of Mimi + LM + State from disk because moshi's
    /// State::reset() does NOT reset model_step_idx, causing RoPE
    /// positional encoding to start at the wrong offset with empty KV caches.
    /// Teardown and rebuild use separate autorelease pools so stale Metal
    /// objects are drained before a fresh device/model is created.
    ///
    /// Mid-session freezes should use soft `refresh_loaded` instead; this
    /// full rebuild remains for session boundaries and diarize mode changes.
    pub fn reset_state(&mut self) -> Result<(), EngineError> {
        FRAME_COUNT.store(0, Ordering::Relaxed);
        if let Ok(mut dbg) = DEBUG_SAMPLES.lock() {
            *dbg = None;
        }

        {
            let loaded = self.model.as_ref().ok_or(EngineError::NotInitialized)?;
            Self::synchronize_device(&loaded.device, "Metal sync before reset")?;
        }

        // Captured before the rebuild closure moves the model fields.
        let batch_size = self.batch_size();
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
            Self::build_loaded_model(device, model_path, config, text_tokenizer, batch_size)
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

        // Initial load is always single-stream; diarization is enabled later via
        // set_diarization + reset_state.
        let batch_size = self.batch_size();
        let loaded = with_autorelease_pool(move || {
            Self::build_loaded_model(
                device,
                model_path.to_path_buf(),
                config,
                text_tokenizer,
                batch_size,
            )
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

        // Process audio in MIMI_FRAME_SIZE-sample frames (80ms at 24kHz).
        // Soft context refresh + silence prefix are handled per-frame so a
        // mid-session KV clear can re-anchor before the next real samples.
        for chunk in audio.chunks(MIMI_FRAME_SIZE) {
            Self::maybe_refresh_before_frame(model)?;
            if model.prefix_pending {
                Self::feed_silence_prefix(model, &device, debug_enabled, &mut segments)?;
            }

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

            let asr_msgs =
                Self::step_pcm_single(model, &device, chunk_data, debug_enabled)?;
            Self::consume_asr_msgs(model, &asr_msgs, debug_enabled, &mut segments);
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
        let suffix_seconds = model.config.stt_config.audio_delay_seconds + 1.0;
        let silence_samples = (suffix_seconds * SAMPLE_RATE as f64) as usize;
        let diarize = self.diarize;
        let pause_streak = model.vad_pause_streak;

        if !diarize && pause_streak >= delay_frames + VAD_FLUSH_MARGIN_FRAMES {
            info!(
                streak = pause_streak,
                delay_frames, "VAD pause covers ASR delay, skipping silence flush"
            );
            return Ok(Vec::new());
        }

        // Feed silence suffix to push any remaining words out of the model's
        // internal pipeline (audio_delay + 1 second of silence). Both lanes get
        // the same silence in diarized mode.
        let silence = vec![0.0f32; silence_samples];
        if diarize {
            self.transcribe_dual(&silence, &silence)
        } else {
            self.transcribe(&silence, None)
        }
    }

    fn reset_state(&mut self) -> Result<(), EngineError> {
        KyutaiEngine::reset_state(self)
    }

    fn supports_diarization(&self) -> bool {
        true
    }

    fn set_diarization(&mut self, enabled: bool) {
        self.diarize = enabled;
    }

    fn transcribe_dual(
        &mut self,
        me: &[f32],
        them: &[f32],
    ) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let debug_enabled = crate::debug::transcription_debug_enabled();
        let model = self.model.as_mut().ok_or(EngineError::NotInitialized)?;
        let device = model.device.clone();
        let mut segments = Vec::new();

        // Both lanes step together; cover whichever is longer (the mixer keeps
        // them equal, but pad defensively).
        let frame_count = me
            .len()
            .div_ceil(MIMI_FRAME_SIZE)
            .max(them.len().div_ceil(MIMI_FRAME_SIZE));

        for f in 0..frame_count {
            Self::maybe_refresh_before_frame(model)?;
            if model.prefix_pending {
                Self::feed_silence_prefix(model, &device, debug_enabled, &mut segments)?;
            }

            let mut data = Vec::with_capacity(2 * MIMI_FRAME_SIZE);
            data.extend_from_slice(&frame_at(me, f));
            data.extend_from_slice(&frame_at(them, f));

            let asr_msgs = Self::step_pcm_dual(model, &device, &data)?;
            Self::consume_asr_msgs(model, &asr_msgs, debug_enabled, &mut segments);
        }

        Ok(segments)
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

    fn emission_delay_seconds(&self) -> f64 {
        self.model
            .as_ref()
            .map(|m| m.config.stt_config.audio_delay_seconds)
            .unwrap_or(0.0)
    }

    fn normalize_text(&self, text: &str) -> String {
        // SentencePiece uses ▁ (U+2581) as word-boundary marker.
        // Replace with space, then trim/collapse.
        let normalized = text.replace('▁', " ");
        collapse_whitespace(&normalized)
    }

    fn context_window_stats(&self) -> Option<super::ContextWindowStats> {
        KyutaiEngine::context_window_stats(self)
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

    #[test]
    fn should_refresh_none_before_soft_window() {
        // stt-2.6b context = 375 → soft at 225, hard at 350
        assert_eq!(should_refresh(100, 375, true), None);
        assert_eq!(should_refresh(224, 375, true), None);
        assert_eq!(should_refresh(225, 375, false), None);
    }

    #[test]
    fn should_refresh_soft_pause_at_60_percent_context() {
        assert_eq!(
            should_refresh(225, 375, true),
            Some(RefreshKind::SoftPause)
        );
    }

    #[test]
    fn should_refresh_hard_deadline_near_context() {
        assert_eq!(
            should_refresh(350, 375, false),
            Some(RefreshKind::HardDeadline)
        );
        assert_eq!(
            should_refresh(350, 375, true),
            Some(RefreshKind::HardDeadline)
        );
    }

    #[test]
    fn should_refresh_ignores_zero_context_or_fresh_epoch() {
        assert_eq!(should_refresh(400, 0, true), None);
        assert_eq!(should_refresh(0, 375, true), None);
    }

    #[test]
    fn word_start_time_keeps_epochs_monotone() {
        // moshi_start within epoch, minus prefix, plus prior epochs.
        assert_eq!(KyutaiEngine::word_start_time_raw(2.0, 1.0, 30.0), 31.0);
        // Prefix still draining: clamp at 0.
        assert_eq!(KyutaiEngine::word_start_time_raw(0.5, 1.0, 0.0), 0.0);
    }
}
