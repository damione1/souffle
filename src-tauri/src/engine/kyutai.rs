use std::fs::File;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use candle_core::{Device, Tensor};
use tracing::{debug, info, trace, warn};

use super::{
    AudioInputRequirements, EngineError, Speaker, TranscriptionEngine, TranscriptionSegment,
    collapse_whitespace,
};
use crate::constants::{MIMI_FRAME_SIZE, MIMI_FRAMES_PER_SECOND, SAMPLE_RATE};
use crate::lid::{LanguageTracker, detect_word};
use crate::platform::with_autorelease_pool;
use crate::settings::MeetingTranscriptionLanguage;

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
pub enum RefreshKind {
    /// Pause-aligned refresh inside the soft context window (preferred).
    SoftPause,
    /// Forced refresh before the LM context window saturates.
    HardDeadline,
    /// Per-lane reset triggered by consecutive language mismatches.
    LanguageMismatch,
}

/// Whether to clear KV cache and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshDecision {
    None,
    Full(RefreshKind),
    Lane { batch_idx: usize, kind: RefreshKind },
}

/// Decide proactive KV refresh before the next frame.
fn decide_refresh(
    frames_since_refresh: usize,
    context: usize,
    vad_pause_streak: &[usize],
    batch_size: usize,
) -> RefreshDecision {
    if context == 0 || frames_since_refresh == 0 {
        return RefreshDecision::None;
    }
    let soft = (context * REFRESH_SOFT_CONTEXT_NUM) / REFRESH_SOFT_CONTEXT_DEN;
    let hard = context.saturating_sub(REFRESH_HARD_MARGIN_FRAMES);

    if frames_since_refresh >= hard {
        return RefreshDecision::Full(RefreshKind::HardDeadline);
    }

    if frames_since_refresh < soft {
        return RefreshDecision::None;
    }

    let pausing: Vec<usize> = vad_pause_streak
        .iter()
        .enumerate()
        .filter(|(_, streak)| **streak >= REFRESH_PAUSE_FRAMES)
        .map(|(idx, _)| idx)
        .collect();

    if pausing.is_empty() {
        return RefreshDecision::None;
    }

    if batch_size == 1 {
        return RefreshDecision::Full(RefreshKind::SoftPause);
    }

    if pausing.len() == 1 {
        let paused = pausing[0];
        let other = 1 - paused;
        if vad_pause_streak.get(other).copied().unwrap_or(0) < REFRESH_PAUSE_FRAMES {
            return RefreshDecision::Lane {
                batch_idx: paused,
                kind: RefreshKind::SoftPause,
            };
        }
    }

    RefreshDecision::Full(RefreshKind::SoftPause)
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
    /// Prefix duration for the current epoch, per batch lane; subtracted from
    /// moshi times.
    time_offset_seconds: Vec<f64>,
    /// Wall-clock seconds of real audio attributed to prior epochs of each lane,
    /// so Word timestamps stay monotone across KV clears. Per lane because
    /// `refresh_lane` restarts one lane's moshi clock and not the other's.
    epoch_origin_seconds: Vec<f64>,
    /// LM frames fed to each lane since that lane's last KV clear, full or
    /// per-lane. This is what a lane's epoch credit is computed from.
    frames_since_lane_reset: Vec<usize>,
    /// Last start_time emitted per lane. Guarantees monotonicity even if a
    /// lane's internal clock restarts (reset_batch_idx).
    last_emitted_start: Vec<f64>,
    /// LM frames since the last soft/hard full refresh (includes this epoch's
    /// prefix). Drives the refresh policy and the hard context deadline.
    frames_since_refresh: usize,
    /// Soft context refreshes performed this session (diagnostics).
    refresh_count: u64,
    /// Consecutive frames where the semantic VAD pause head fired, per batch lane.
    vad_pause_streak: Vec<usize>,
    /// Per-lane LID and mismatch streak tracking.
    language_tracker: LanguageTracker,
    /// Word waiting for its EndWord (or the next Word) before emit, per lane.
    pending_words: Vec<Option<PendingWord>>,
    /// Pending words closed by a KV refresh, emitted on the next consume.
    orphaned_words: Vec<PendingWord>,
}

/// A decoded word held until moshi emits `EndWord` so `end_time` is real.
#[derive(Clone)]
struct PendingWord {
    text: String,
    start_time: f64,
    language: Option<String>,
    speaker: Option<Speaker>,
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
    /// Heuristic prior for LID/mismatch resets (never passed to moshi).
    meeting_language_prior: MeetingTranscriptionLanguage,
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
            meeting_language_prior: MeetingTranscriptionLanguage::Auto,
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
        meeting_language_prior: MeetingTranscriptionLanguage,
    ) -> Result<LoadedModel, EngineError> {
        let state = Self::build_state(&device, &model_path, &config, batch_size)?;
        Ok(LoadedModel {
            state,
            text_tokenizer,
            config,
            device,
            model_path,
            prefix_pending: true,
            time_offset_seconds: vec![0.0; batch_size],
            epoch_origin_seconds: vec![0.0; batch_size],
            frames_since_lane_reset: vec![0; batch_size],
            last_emitted_start: vec![0.0; batch_size],
            frames_since_refresh: 0,
            refresh_count: 0,
            vad_pause_streak: vec![0; batch_size],
            language_tracker: LanguageTracker::new(batch_size, meeting_language_prior),
            pending_words: vec![None; batch_size],
            orphaned_words: Vec::new(),
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
    fn word_start_time(model: &LoadedModel, moshi_start: f64, batch_idx: usize) -> f64 {
        Self::word_start_time_raw(
            moshi_start,
            model
                .time_offset_seconds
                .get(batch_idx)
                .copied()
                .unwrap_or(0.0),
            model
                .epoch_origin_seconds
                .get(batch_idx)
                .copied()
                .unwrap_or(0.0),
        )
    }

    fn word_start_time_raw(
        moshi_start: f64,
        time_offset_seconds: f64,
        epoch_origin_seconds: f64,
    ) -> f64 {
        (moshi_start - time_offset_seconds + epoch_origin_seconds).max(0.0)
    }

    /// Floor an emitted timestamp at the last one emitted for the same lane.
    /// A regression here can only mean a lane's moshi clock restarted without
    /// its epoch being credited; the warning is the only signal that would
    /// surface such a bug, since the UI would just silently rewind.
    fn monotonic_time(model: &mut LoadedModel, batch_idx: usize, raw: f64) -> f64 {
        let Some(floor) = model.last_emitted_start.get_mut(batch_idx) else {
            return raw;
        };
        if raw < *floor - 1.0 {
            warn!(
                batch_idx,
                raw = format!("{raw:.2}"),
                floor = format!("{:.2}", *floor),
                drop = format!("{:.2}", *floor - raw),
                "Lane timestamp regressed; clamping to keep the transcript monotone"
            );
        }
        let clamped = raw.max(*floor);
        *floor = clamped;
        clamped
    }

    /// Credit a lane's elapsed audio to its epoch origin and restart its clock.
    /// Moshi's `reset_batch_idx` zeroes that lane's `step_idx` and
    /// `last_stop_time`, so without this credit the lane's next word maps back
    /// onto the start of the epoch and the transcript rewinds by up to a full
    /// context window.
    fn credit_lane_epoch(
        epoch_origin_seconds: &mut [f64],
        time_offset_seconds: &mut [f64],
        frames_since_lane_reset: &mut [usize],
        lane: usize,
    ) {
        let (Some(origin), Some(offset), Some(frames)) = (
            epoch_origin_seconds.get_mut(lane),
            time_offset_seconds.get_mut(lane),
            frames_since_lane_reset.get_mut(lane),
        ) else {
            return;
        };
        let lane_secs = *frames as f64 / MIMI_FRAMES_PER_SECOND - *offset;
        *origin += lane_secs.max(0.0);
        *offset = 0.0;
        *frames = 0;
    }

    fn pending_to_segment(pending: PendingWord, end_time: f64) -> TranscriptionSegment {
        TranscriptionSegment {
            text: pending.text,
            start_time: pending.start_time,
            end_time: end_time.max(pending.start_time),
            is_final: true,
            language: pending.language,
            confidence: None,
            speaker: pending.speaker,
        }
    }

    fn emit_pending(
        pending_words: &mut [Option<PendingWord>],
        batch_idx: usize,
        end_time: f64,
        segments: &mut Vec<TranscriptionSegment>,
    ) {
        if let Some(pending) = pending_words.get_mut(batch_idx).and_then(Option::take) {
            segments.push(Self::pending_to_segment(pending, end_time));
        }
    }

    fn drain_orphans(model: &mut LoadedModel, segments: &mut Vec<TranscriptionSegment>) {
        for pending in model.orphaned_words.drain(..) {
            let end = pending.start_time;
            segments.push(Self::pending_to_segment(pending, end));
        }
    }

    fn drain_all_pending(model: &mut LoadedModel, segments: &mut Vec<TranscriptionSegment>) {
        Self::drain_orphans(model, segments);
        for batch_idx in 0..model.pending_words.len() {
            if let Some(pending) = model.pending_words[batch_idx].take() {
                let end = pending.start_time;
                segments.push(Self::pending_to_segment(pending, end));
            }
        }
    }

    /// Soft KV-cache clear: empties LM/Mimi/ItemState without rebuilding Metal
    /// devices or remapping weights. Preferred over full `reset_state` mid-session.
    fn refresh_loaded(model: &mut LoadedModel, kind: RefreshKind) -> Result<(), EngineError> {
        let frames = model.frames_since_refresh;
        let context = model.config.context;
        for lane in 0..model.epoch_origin_seconds.len() {
            Self::credit_lane_epoch(
                &mut model.epoch_origin_seconds,
                &mut model.time_offset_seconds,
                &mut model.frames_since_lane_reset,
                lane,
            );
        }
        model
            .state
            .reset()
            .map_err(|e| EngineError::InferenceError(format!("ASR context refresh: {e}")))?;
        model.prefix_pending = true;
        model.frames_since_refresh = 0;
        model.vad_pause_streak = vec![0; model.state.batch_size()];
        model.language_tracker.reset_all();
        let mut orphaned: Vec<PendingWord> = Vec::new();
        for pending in &mut model.pending_words {
            if let Some(word) = pending.take() {
                orphaned.push(word);
            }
        }
        model.orphaned_words.append(&mut orphaned);
        model.refresh_count = model.refresh_count.saturating_add(1);
        info!(
            kind = ?kind,
            frames_before_refresh = frames,
            context,
            refresh_count = model.refresh_count,
            epoch_origin_seconds = model
                .epoch_origin_seconds
                .iter()
                .map(|s| format!("{s:.2}"))
                .collect::<Vec<_>>()
                .join(","),
            "ASR context refreshed (soft KV clear)"
        );
        Ok(())
    }

    /// Per-lane KV clear via moshi `reset_batch_idx`. Does not rebuild Metal or
    /// re-feed the silence prefix; the other lane keeps decoding uninterrupted.
    fn refresh_lane(
        model: &mut LoadedModel,
        batch_idx: usize,
        kind: RefreshKind,
    ) -> Result<(), EngineError> {
        // This path feeds no silence prefix, so the lane's offset goes to zero
        // along with its clock.
        Self::credit_lane_epoch(
            &mut model.epoch_origin_seconds,
            &mut model.time_offset_seconds,
            &mut model.frames_since_lane_reset,
            batch_idx,
        );
        model
            .state
            .reset_batch_idx(batch_idx)
            .map_err(|e| EngineError::InferenceError(format!("ASR lane reset: {e}")))?;
        if let Some(streak) = model.vad_pause_streak.get_mut(batch_idx) {
            *streak = 0;
        }
        model.language_tracker.reset_lane(batch_idx);
        let pending = model
            .pending_words
            .get_mut(batch_idx)
            .and_then(Option::take);
        if let Some(pending) = pending {
            model.orphaned_words.push(pending);
        }
        debug!(
            kind = ?kind,
            batch_idx,
            epoch_origin_seconds = format!(
                "{:.2}",
                model.epoch_origin_seconds.get(batch_idx).copied().unwrap_or(0.0)
            ),
            "ASR lane context reset (per-batch KV clear)"
        );
        Ok(())
    }

    fn maybe_refresh_before_frame(model: &mut LoadedModel) -> Result<(), EngineError> {
        let batch_size = model.state.batch_size();
        match decide_refresh(
            model.frames_since_refresh,
            model.config.context,
            &model.vad_pause_streak,
            batch_size,
        ) {
            RefreshDecision::None => {}
            RefreshDecision::Full(kind) => Self::refresh_loaded(model, kind)?,
            RefreshDecision::Lane { batch_idx, kind } => {
                Self::refresh_lane(model, batch_idx, kind)?
            }
        }
        Ok(())
    }

    fn emission_delay_frames(model: &LoadedModel) -> usize {
        (model.config.stt_config.audio_delay_seconds * MIMI_FRAMES_PER_SECOND) as usize
    }

    /// Whether the engine's own semantic VAD has paused long enough that
    /// every word already spoken has had time to clear the pipeline: a pause
    /// streak covering the emission delay, plus margin. Shared by `flush`
    /// (skip the silence suffix) and the `tail_drained` trait method (cut a
    /// single-stream drain window short) so the two can't drift apart.
    ///
    /// Diarized mode always returns false: lane 0's pause streak says
    /// nothing about lane 1 (system audio), and `DiarizedMode` doesn't
    /// consult this signal anyway since both lanes must stay frame-aligned
    /// regardless of either side's pause state.
    fn tail_drained_for(model: &LoadedModel, diarize: bool) -> bool {
        if diarize {
            return false;
        }
        let delay_frames = Self::emission_delay_frames(model);
        let pause_streak = model.vad_pause_streak.first().copied().unwrap_or(0);
        pause_streak >= delay_frames + VAD_FLUSH_MARGIN_FRAMES
    }

    fn note_vad_pause(model: &mut LoadedModel, prs: &[Vec<f32>]) {
        if let Some(pause_head) = prs.get(VAD_PAUSE_HEAD) {
            for (batch_idx, p) in pause_head.iter().enumerate() {
                if let Some(streak) = model.vad_pause_streak.get_mut(batch_idx) {
                    if *p > VAD_PAUSE_THRESHOLD {
                        *streak += 1;
                    } else {
                        *streak = 0;
                    }
                }
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
            model.time_offset_seconds.fill(0.0);
            return Ok(());
        }
        // The prefix is fed to every lane, and its frames also count into
        // `frames_since_lane_reset`, which is what makes subtracting the offset
        // when crediting an epoch the right thing to do.
        let prefix_seconds = prefix_frames as f64 / MIMI_FRAMES_PER_SECOND;
        model.time_offset_seconds.fill(prefix_seconds);
        info!(
            frames = prefix_frames,
            seconds = prefix_seconds,
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
        // One step_pcm advances every lane, so every lane's clock advances.
        for frames in model.frames_since_lane_reset.iter_mut() {
            *frames = frames.saturating_add(1);
        }
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
        // One step_pcm advances every lane, so every lane's clock advances.
        for frames in model.frames_since_lane_reset.iter_mut() {
            *frames = frames.saturating_add(1);
        }
        FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
        Ok(asr_msgs)
    }

    fn consume_asr_msgs(
        model: &mut LoadedModel,
        asr_msgs: &[moshi::asr::AsrMsg],
        debug_enabled: bool,
        segments: &mut Vec<TranscriptionSegment>,
    ) {
        Self::drain_orphans(model, segments);
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
                        debug!(target: crate::logging::TRANSCRIPT_TARGET, tokens = ?tokens, text = ?text, t = format!("{start_time:.2}"), "WORD emitted");
                    }
                    if text.is_empty() {
                        continue;
                    }
                    let language = detect_word(&text).map(|code| code.as_str().to_string());
                    // Mapped before the mismatch reset below: this word's moshi
                    // clock belongs to the epoch that reset is about to close.
                    let raw_start = Self::word_start_time(model, *start_time, *batch_idx);
                    let start_time = Self::monotonic_time(model, *batch_idx, raw_start);
                    let mismatch_reset = model.language_tracker.on_word(&text, *batch_idx);
                    if mismatch_reset
                        && let Err(e) =
                            Self::refresh_lane(model, *batch_idx, RefreshKind::LanguageMismatch)
                    {
                        warn!(batch_idx, "Language mismatch lane reset failed: {e}");
                    }
                    let speaker = if diarized {
                        Some(if *batch_idx == 0 {
                            Speaker::Me
                        } else {
                            Speaker::Them
                        })
                    } else {
                        None
                    };
                    // Previous word on this lane never got EndWord: close it
                    // at this word's start so we don't drop it.
                    Self::emit_pending(&mut model.pending_words, *batch_idx, start_time, segments);
                    if *batch_idx >= model.pending_words.len() {
                        model.pending_words.resize_with(*batch_idx + 1, || None);
                    }
                    model.pending_words[*batch_idx] = Some(PendingWord {
                        text,
                        start_time,
                        language,
                        speaker,
                    });
                }
                moshi::asr::AsrMsg::EndWord {
                    stop_time,
                    batch_idx,
                } => {
                    let raw_end = Self::word_start_time(model, *stop_time, *batch_idx);
                    let end_time = Self::monotonic_time(model, *batch_idx, raw_end);
                    Self::emit_pending(&mut model.pending_words, *batch_idx, end_time, segments);
                }
                moshi::asr::AsrMsg::Step { prs, .. } => {
                    Self::note_vad_pause(model, prs);
                }
            }
        }
        Self::drain_orphans(model, segments);
    }

    fn context_window_stats(&self) -> Option<super::ContextWindowStats> {
        self.model.as_ref().map(|m| super::ContextWindowStats {
            context_frames: m.config.context,
            frames_since_refresh: m.frames_since_refresh,
            refresh_count: m.refresh_count,
        })
    }

    pub fn set_meeting_language_prior(&mut self, prior: MeetingTranscriptionLanguage) {
        self.meeting_language_prior = prior;
        if let Some(model) = self.model.as_mut() {
            let batch_size = model.state.batch_size();
            model
                .language_tracker
                .resize(batch_size, self.meeting_language_prior);
        }
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
        let meeting_language_prior = self.meeting_language_prior;
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
            Self::build_loaded_model(
                device,
                model_path,
                config,
                text_tokenizer,
                batch_size,
                meeting_language_prior,
            )
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
        let meeting_language_prior = self.meeting_language_prior;
        let loaded = with_autorelease_pool(move || {
            Self::build_loaded_model(
                device,
                model_path.to_path_buf(),
                config,
                text_tokenizer,
                batch_size,
                meeting_language_prior,
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

            let asr_msgs = Self::step_pcm_single(model, &device, chunk_data, debug_enabled)?;
            Self::consume_asr_msgs(model, &asr_msgs, debug_enabled, &mut segments);
        }

        Ok(segments)
    }

    fn flush(&mut self) -> Result<Vec<TranscriptionSegment>, EngineError> {
        let diarize = self.diarize;
        let (delay_frames, suffix_seconds, pause_streak, drained) = {
            let model = self.model.as_ref().ok_or(EngineError::NotInitialized)?;
            let delay_frames = Self::emission_delay_frames(model);
            let suffix_seconds = model.config.stt_config.audio_delay_seconds + 1.0;
            let pause_streak = model.vad_pause_streak.first().copied().unwrap_or(0);
            let drained = Self::tail_drained_for(model, diarize);
            (delay_frames, suffix_seconds, pause_streak, drained)
        };
        let silence_samples = (suffix_seconds * SAMPLE_RATE as f64) as usize;

        // Words are emitted audio_delay after they are spoken. If the semantic
        // VAD has reported a pause for longer than that delay (plus margin),
        // every word has already cleared the pipeline and the silence suffix
        // would only burn inference time at stop.
        if drained {
            info!(
                streak = pause_streak,
                delay_frames, "VAD pause covers ASR delay, skipping silence flush"
            );
            let mut segments = Vec::new();
            if let Some(model) = self.model.as_mut() {
                Self::drain_all_pending(model, &mut segments);
            }
            return Ok(segments);
        }

        // Feed silence suffix to push any remaining words out of the model's
        // internal pipeline (audio_delay + 1 second of silence). Both lanes get
        // the same silence in diarized mode.
        let silence = vec![0.0f32; silence_samples];
        let mut segments = if diarize {
            self.transcribe_dual(&silence, &silence)?
        } else {
            self.transcribe(&silence, None)?
        };
        if let Some(model) = self.model.as_mut() {
            Self::drain_all_pending(model, &mut segments);
        }
        Ok(segments)
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

    fn set_meeting_language_prior(&mut self, prior: crate::settings::MeetingTranscriptionLanguage) {
        KyutaiEngine::set_meeting_language_prior(self, prior);
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

    fn tail_drained(&self) -> bool {
        self.model
            .as_ref()
            .map(|m| Self::tail_drained_for(m, self.diarize))
            .unwrap_or(false)
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
    fn decide_refresh_none_before_soft_window() {
        let streak = [0usize];
        assert_eq!(decide_refresh(100, 375, &streak, 1), RefreshDecision::None);
        assert_eq!(decide_refresh(224, 375, &streak, 1), RefreshDecision::None);
        assert_eq!(decide_refresh(225, 375, &[0], 1), RefreshDecision::None);
    }

    #[test]
    fn decide_refresh_soft_pause_at_60_percent_context() {
        assert_eq!(
            decide_refresh(225, 375, &[8], 1),
            RefreshDecision::Full(RefreshKind::SoftPause)
        );
    }

    #[test]
    fn decide_refresh_hard_deadline_near_context() {
        assert_eq!(
            decide_refresh(350, 375, &[0], 1),
            RefreshDecision::Full(RefreshKind::HardDeadline)
        );
        assert_eq!(
            decide_refresh(350, 375, &[8], 1),
            RefreshDecision::Full(RefreshKind::HardDeadline)
        );
    }

    #[test]
    fn decide_refresh_ignores_zero_context_or_fresh_epoch() {
        assert_eq!(decide_refresh(400, 0, &[8], 1), RefreshDecision::None);
        assert_eq!(decide_refresh(0, 375, &[8], 1), RefreshDecision::None);
    }

    #[test]
    fn decide_refresh_dual_one_lane_paused_other_active() {
        assert_eq!(
            decide_refresh(300, 375, &[8, 2], 2),
            RefreshDecision::Lane {
                batch_idx: 0,
                kind: RefreshKind::SoftPause,
            }
        );
    }

    #[test]
    fn decide_refresh_dual_both_paused_full_refresh() {
        assert_eq!(
            decide_refresh(300, 375, &[8, 10], 2),
            RefreshDecision::Full(RefreshKind::SoftPause)
        );
    }

    #[test]
    fn word_start_time_keeps_epochs_monotone() {
        // moshi_start within epoch, minus prefix, plus prior epochs.
        assert_eq!(KyutaiEngine::word_start_time_raw(2.0, 1.0, 30.0), 31.0);
        // Prefix still draining: clamp at 0.
        assert_eq!(KyutaiEngine::word_start_time_raw(0.5, 1.0, 0.0), 0.0);
    }

    #[test]
    fn credit_lane_epoch_keeps_the_reset_lane_monotone() {
        // Two lanes 24s into an epoch that opened with a 13-frame (1.04s)
        // silence prefix, 58s of earlier epochs already credited.
        let mut origin = [58.0f64, 58.0];
        let mut offset = [1.04f64, 1.04];
        let mut frames = [300usize, 300];

        let last_emitted = KyutaiEngine::word_start_time_raw(
            (300 - 13) as f64 / crate::constants::MIMI_FRAMES_PER_SECOND,
            offset[0],
            origin[0],
        );

        KyutaiEngine::credit_lane_epoch(&mut origin, &mut offset, &mut frames, 0);

        // reset_batch_idx restarts the lane's moshi clock at 0: the next word
        // must not land before the last one emitted on that lane.
        let after_reset = KyutaiEngine::word_start_time_raw(0.0, offset[0], origin[0]);
        assert!(
            after_reset >= last_emitted,
            "lane 0 rewound: {after_reset} < {last_emitted}"
        );
        assert_eq!(frames[0], 0);
        assert_eq!(offset[0], 0.0);
    }

    #[test]
    fn credit_lane_epoch_leaves_the_other_lane_untouched() {
        let mut origin = [58.0f64, 58.0];
        let mut offset = [1.04f64, 1.04];
        let mut frames = [300usize, 300];

        KyutaiEngine::credit_lane_epoch(&mut origin, &mut offset, &mut frames, 0);

        // The lane that kept decoding must keep its clock: crediting it here
        // would push its words forward by a whole epoch.
        assert_eq!(origin[1], 58.0);
        assert_eq!(offset[1], 1.04);
        assert_eq!(frames[1], 300);
    }

    #[test]
    fn pending_word_uses_endword_stop_time() {
        let pending = PendingWord {
            text: "hello".into(),
            start_time: 1.2,
            language: Some("en".into()),
            speaker: Some(Speaker::Me),
        };
        let seg = KyutaiEngine::pending_to_segment(pending, 1.6);
        assert_eq!(seg.text, "hello");
        assert_eq!(seg.start_time, 1.2);
        assert_eq!(seg.end_time, 1.6);
        assert_eq!(seg.speaker, Some(Speaker::Me));
        assert!(seg.is_final);
    }

    #[test]
    fn pending_word_end_time_never_precedes_start() {
        let pending = PendingWord {
            text: "hi".into(),
            start_time: 2.0,
            language: None,
            speaker: None,
        };
        let seg = KyutaiEngine::pending_to_segment(pending, 1.5);
        assert_eq!(seg.end_time, 2.0);
    }
}
