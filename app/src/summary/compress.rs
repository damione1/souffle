//! LLMLingua-2 encoder/head with versioned extractive byte-span selection.
use crate::{
    models::compression::{self, ModelStatus},
    transcript::{CompactTranscript, MeetingTranscript},
};
use candle_core::{DType, Device, Tensor};
use candle_nn::{Module, VarBuilder};
use candle_transformers::models::bert::{BertEncoder, Config};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

#[derive(Debug)]
pub enum CompressionFailure {
    ModelValidation { detail: String },
    Inference { detail: String },
}
impl From<String> for CompressionFailure {
    fn from(detail: String) -> Self {
        Self::Inference { detail }
    }
}
impl From<&str> for CompressionFailure {
    fn from(detail: &str) -> Self {
        Self::Inference {
            detail: detail.into(),
        }
    }
}
impl std::fmt::Display for CompressionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModelValidation { detail } | Self::Inference { detail } => f.write_str(detail),
        }
    }
}
impl std::error::Error for CompressionFailure {}

pub const POLICY: &str = "mbert-offset-spans-v1";
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompressionRate(f32);
impl CompressionRate {
    pub const MIN: f32 = 0.3;
    pub const MAX: f32 = 0.9;
    pub const DEFAULT: f32 = 0.5;
    pub fn new(rate: f32) -> Result<Self, String> {
        if rate.is_finite() && (Self::MIN..=Self::MAX).contains(&rate) {
            Ok(Self(rate))
        } else {
            Err("Compression rate must be finite and between 0.3 and 0.9".into())
        }
    }
    pub fn get(self) -> f32 {
        self.0
    }
}
#[derive(Debug, Clone)]
pub struct CompressionInput {
    pub lines: Vec<String>,
    pub participants: Vec<String>,
    pub source_sha256: String,
    pub segmented: bool,
}
impl CompressionInput {
    pub fn from_meeting(meeting: &MeetingTranscript) -> Self {
        let edited = meeting
            .edited_transcript
            .as_deref()
            .filter(|s| !s.is_empty());
        let lines = edited
            .map(|s| s.split('\n').map(str::to_owned).collect())
            .unwrap_or_else(|| crate::summary::turns_from_segments(&meeting.segments));
        Self::new(
            lines,
            meeting
                .participants
                .iter()
                .map(|p| p.name.clone())
                .collect(),
            &meeting.recording_sessions,
            meeting.ended_at.is_none(),
            edited.is_none(),
        )
    }
    pub(crate) fn new(
        lines: Vec<String>,
        participants: Vec<String>,
        sessions: &impl serde::Serialize,
        ongoing: bool,
        segmented: bool,
    ) -> Self {
        let canonical = serde_json::to_vec(&(&lines, &participants, sessions, ongoing, segmented))
            .expect("serializable source");
        Self {
            lines,
            participants,
            source_sha256: format!("{:x}", Sha256::digest(canonical)),
            segmented,
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub enum CompactJobStatus {
    Idle,
    WaitingForModel,
    Queued,
    Compressing { completed: usize, total: usize },
    Failed { detail: String },
    SourceChanged,
    InterruptedByRecording,
}
#[derive(Clone)]
pub struct CompactJob {
    pub generation: String,
    pub cancel: Arc<AtomicBool>,
    pub status: CompactJobStatus,
}
struct InstallationAttempt {
    cancel: Arc<AtomicBool>,
    result: Mutex<Option<Result<(), String>>>,
    done: tokio::sync::Notify,
    recording_epoch: u64,
}
/// Coordinates workers only. Models, devices and tokenizers belong to blocking scopes.
pub struct CompressionService {
    pub(crate) compute: Arc<tokio::sync::Semaphore>,
    install: Mutex<Option<Arc<InstallationAttempt>>>,
    model: Mutex<ModelStatus>,
    jobs: Mutex<HashMap<String, CompactJob>>,
    requests: Mutex<HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>,
    recording_epoch: AtomicU64,
    pub model_dir: PathBuf,
}
impl CompressionService {
    pub fn shared() -> Arc<Self> {
        static SERVICE: OnceLock<Arc<CompressionService>> = OnceLock::new();
        SERVICE
            .get_or_init(|| Arc::new(Self::new(compression::model_dir())))
            .clone()
    }
    pub fn new(model_dir: PathBuf) -> Self {
        let status = compression::installed(&model_dir)
            .map(|m| ModelStatus::Ready { bytes: m.bytes })
            .unwrap_or(ModelStatus::NotDownloaded);
        Self {
            compute: Arc::new(tokio::sync::Semaphore::new(1)),
            install: Mutex::new(None),
            model: Mutex::new(status),
            jobs: Mutex::new(HashMap::new()),
            requests: Mutex::new(HashMap::new()),
            recording_epoch: AtomicU64::new(0),
            model_dir,
        }
    }
    pub(crate) fn meeting_request(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut requests = self.requests.lock().unwrap_or_else(|p| p.into_inner());
        requests.retain(|_, request| request.strong_count() > 0);
        if let Some(request) = requests.get(id).and_then(std::sync::Weak::upgrade) {
            return request;
        }
        let request = Arc::new(tokio::sync::Mutex::new(()));
        requests.insert(id.into(), Arc::downgrade(&request));
        request
    }
    pub fn model_status(&self) -> ModelStatus {
        self.model.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
    pub(crate) fn set_model(&self, status: ModelStatus) {
        *self.model.lock().unwrap_or_else(|p| p.into_inner()) = status;
    }
    pub fn job(&self, id: &str) -> Option<CompactJob> {
        self.jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned()
    }
    pub fn start_job(&self, id: &str) -> CompactJob {
        let mut jobs = self.jobs.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(old) = jobs.get(id) {
            old.cancel.store(true, Ordering::Relaxed);
        }
        let job = CompactJob {
            generation: uuid::Uuid::new_v4().to_string(),
            cancel: Arc::new(AtomicBool::new(false)),
            status: CompactJobStatus::Queued,
        };
        jobs.insert(id.into(), job.clone());
        job
    }
    pub fn update_job(&self, id: &str, generation: &str, status: CompactJobStatus) {
        if let Some(job) = self
            .jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(id)
            .filter(|j| j.generation == generation && !j.cancel.load(Ordering::Relaxed))
        {
            job.status = status;
        }
    }
    pub fn cancel(&self, id: &str) {
        if let Some(job) = self
            .jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(id)
        {
            job.cancel.store(true, Ordering::Relaxed);
            job.status = CompactJobStatus::Idle;
        }
    }
    pub fn cancel_install(&self) {
        if let Some(attempt) = self
            .install
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            attempt.cancel.store(true, Ordering::Relaxed);
        }
    }
    pub fn recording_epoch(&self) -> u64 {
        self.recording_epoch.load(Ordering::SeqCst)
    }
    pub fn recording_started(&self) {
        self.recording_epoch.fetch_add(1, Ordering::SeqCst);
        self.cancel_install();
        for job in self
            .jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values_mut()
        {
            match job.status {
                CompactJobStatus::WaitingForModel
                | CompactJobStatus::Queued
                | CompactJobStatus::Compressing { .. } => {
                    job.cancel.store(true, Ordering::Relaxed);
                    job.status = CompactJobStatus::InterruptedByRecording;
                }
                CompactJobStatus::Idle
                | CompactJobStatus::Failed { .. }
                | CompactJobStatus::SourceChanged
                | CompactJobStatus::InterruptedByRecording => {}
            }
        }
    }
    pub async fn ensure_model(
        self: &Arc<Self>,
        state: std::sync::Weak<crate::state::AppState>,
    ) -> Result<(), String> {
        // Every caller overlapping this attempt observes its same outcome. A failed or
        // cancelled attempt is only retried by a later explicit action.
        let (attempt, start) = {
            let mut installed = self.install.lock().unwrap_or_else(|p| p.into_inner());
            match installed.as_ref().filter(|attempt| {
                attempt
                    .result
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .is_none()
            }) {
                Some(attempt) => (attempt.clone(), false),
                None => {
                    let attempt = Arc::new(InstallationAttempt {
                        cancel: Arc::new(AtomicBool::new(false)),
                        result: Mutex::new(None),
                        done: tokio::sync::Notify::new(),
                        recording_epoch: self.recording_epoch(),
                    });
                    *installed = Some(attempt.clone());
                    (attempt, true)
                }
            }
        };
        if start {
            let service = self.clone();
            let attempt = attempt.clone();
            crate::async_runtime::spawn(async move {
                let result = service.run_install(state, &attempt).await;
                *attempt.result.lock().unwrap_or_else(|p| p.into_inner()) = Some(result);
                attempt.done.notify_waiters();
            });
        }
        loop {
            let notified = attempt.done.notified();
            if let Some(result) = attempt
                .result
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
            {
                return result;
            }
            notified.await;
        }
    }
    async fn run_install(
        self: &Arc<Self>,
        state: std::sync::Weak<crate::state::AppState>,
        attempt: &InstallationAttempt,
    ) -> Result<(), String> {
        let idle = || {
            state.upgrade().is_some_and(|state| {
                !crate::commands::settings_capture_busy(&state).unwrap_or(true)
            })
        };
        if self.recording_epoch() != attempt.recording_epoch
            || !idle()
            || attempt.cancel.load(Ordering::Relaxed)
        {
            return Err("Compression interrupted by recording or cancelled".into());
        }
        self.set_model(if compression::installed(&self.model_dir).is_some() {
            ModelStatus::Preparing
        } else {
            ModelStatus::Downloading {
                downloaded: 0,
                total: compression::download_bytes(),
            }
        });
        let service = self.clone();
        let cancel = attempt.cancel.clone();
        let worker_cancel = cancel.clone();
        let result = tokio::task::spawn_blocking(move || {
            compression::install(&service.model_dir, &worker_cancel, |status| {
                if !state.upgrade().is_some_and(|state| {
                    !crate::commands::settings_capture_busy(&state).unwrap_or(true)
                }) {
                    worker_cancel.store(true, Ordering::Relaxed);
                }
                service.set_model(status);
            })
        })
        .await
        .map_err(|e| e.to_string())
        .and_then(|result| result);
        match result {
            Ok(meta) => {
                self.set_model(ModelStatus::Ready { bytes: meta.bytes });
                Ok(())
            }
            Err(error) => {
                self.set_model(if cancel.load(Ordering::Relaxed) {
                    compression::installed(&self.model_dir)
                        .map(|meta| ModelStatus::Ready { bytes: meta.bytes })
                        .unwrap_or(ModelStatus::NotDownloaded)
                } else {
                    ModelStatus::Failed {
                        detail: error.clone(),
                    }
                });
                Err(error)
            }
        }
    }
    pub fn compatible(
        &self,
        compact: &CompactTranscript,
        input: &CompressionInput,
        rate: CompressionRate,
    ) -> bool {
        !compact.is_stale
            && compact.source_sha256 == input.source_sha256
            && compact.rate == rate.get()
            && compact
                .model
                .strip_prefix(&format!(
                    "{}@{};fp16:",
                    compression::REPOSITORY,
                    compression::REVISION
                ))
                .and_then(|value| value.strip_suffix(&format!(";{POLICY}")))
                .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
    }
}
pub fn provenance(meta: &compression::Installation) -> String {
    format!(
        "{}@{};fp16:{};{}",
        meta.repository, meta.revision, meta.sha256, POLICY
    )
}

struct BertClassifier {
    word: candle_nn::Embedding,
    position: candle_nn::Embedding,
    token_type: candle_nn::Embedding,
    norm: candle_nn::LayerNorm,
    encoder: BertEncoder,
    classifier: candle_nn::Linear,
}
impl BertClassifier {
    fn load(vb: VarBuilder<'_>, config: &Config) -> candle_core::Result<Self> {
        let embeddings = vb.pp("bert.embeddings");
        Ok(Self {
            word: candle_nn::embedding(
                config.vocab_size,
                config.hidden_size,
                embeddings.pp("word_embeddings"),
            )?,
            position: candle_nn::embedding(
                config.max_position_embeddings,
                config.hidden_size,
                embeddings.pp("position_embeddings"),
            )?,
            token_type: candle_nn::embedding(
                config.type_vocab_size,
                config.hidden_size,
                embeddings.pp("token_type_embeddings"),
            )?,
            norm: candle_nn::layer_norm(
                config.hidden_size,
                config.layer_norm_eps,
                embeddings.pp("LayerNorm"),
            )?,
            encoder: BertEncoder::load(vb.pp("bert.encoder"), config)?,
            classifier: candle_nn::linear(config.hidden_size, 2, vb.pp("classifier"))?,
        })
    }
    fn forward(&self, ids: &Tensor, mask: &Tensor) -> candle_core::Result<Tensor> {
        let (batch, width) = ids.dims2()?;
        let positions = Tensor::arange(0u32, width as u32, ids.device())?;
        let types = Tensor::zeros((batch, width), DType::U32, ids.device())?;
        let embeddings = self
            .word
            .forward(ids)?
            .broadcast_add(&self.position.forward(&positions)?)?
            .add(&self.token_type.forward(&types)?)?;
        let embeddings = self.norm.forward(&embeddings)?;
        self.classifier
            .forward(&self.encoder.forward(&embeddings, mask)?)
    }
}
struct Window {
    line: usize,
    ids: Vec<u32>,
    offsets: Vec<(usize, usize)>,
}
struct Word {
    start: usize,
    end: usize,
    score: f32,
    count: usize,
    forced: bool,
    keep: bool,
}
fn split_prefix(line: &str) -> (&str, &str) {
    static PREFIX: OnceLock<regex::Regex> = OnceLock::new();
    let re = PREFIX.get_or_init(|| regex::Regex::new(r"^\[\d+:\d{2}\][ \t]*").unwrap());
    let Some(found) = re.find(line) else {
        return ("", line);
    };
    let mut end = found.end();
    if let Some((label, _)) = line[end..].split_once(':') {
        if crate::engine::Speaker::parse(&label.to_lowercase()).is_some() {
            end += label.len() + 1;
            end += line[end..].len() - line[end..].trim_start_matches([' ', '\t']).len();
        }
    }
    line.split_at(end)
}

fn words(body: &str, names: &[String]) -> Vec<Word> {
    let protected: Vec<_> = names
        .iter()
        .filter(|n| !n.trim().is_empty())
        .flat_map(|name| {
            let spans = match regex::RegexBuilder::new(&regex::escape(name.trim()))
                .case_insensitive(true)
                .build()
            {
                Ok(re) => re
                    .find_iter(body)
                    .map(|m| (m.start(), m.end()))
                    .collect::<Vec<_>>(),
                Err(_) => body
                    .match_indices(name.trim())
                    .map(|(start, value)| (start, start + value.len()))
                    .collect(),
            };
            spans
                .into_iter()
                .filter(|&(start, end)| {
                    body[..start]
                        .chars()
                        .next_back()
                        .is_none_or(|c| !c.is_alphanumeric())
                        && body[end..]
                            .chars()
                            .next()
                            .is_none_or(|c| !c.is_alphanumeric())
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let mut result = Vec::new();
    let mut start = None;
    for (i, c) in body
        .char_indices()
        .chain(std::iter::once((body.len(), ' ')))
    {
        if c.is_whitespace() {
            if let Some(a) = start.take() {
                let text = &body[a..i];
                let lower = text
                    .trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’')
                    .to_lowercase();
                let negation = [
                    "ne", "pas", "jamais", "aucun", "aucune", "sans", "non", "rien", "ni", "n'",
                    "n’",
                ]
                .contains(&lower.as_str())
                    || lower.starts_with("n'")
                    || lower.starts_with("n’");
                result.push(Word {
                    start: a,
                    end: i,
                    score: 0.,
                    count: 0,
                    forced: negation
                        || text.chars().any(char::is_numeric)
                        || protected.iter().any(|&(x, y)| a < y && i > x),
                    keep: false,
                });
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    result
}
fn reconstruct(body: &str, words: &[Word]) -> String {
    let mut output = String::new();
    let mut previous = None;
    for word in words.iter().filter(|w| w.keep || w.forced) {
        if let Some(end) = previous {
            if body[end..word.start].chars().all(char::is_whitespace) {
                output.push_str(&body[end..word.start]);
            } else {
                output.push(' ');
            }
        }
        output.push_str(&body[word.start..word.end]);
        previous = Some(word.end);
    }
    output
}
/// Runs entirely on a blocking worker. Returning drops mappings, tokenizer and all GPU resources.
pub fn compress(
    dir: &Path,
    input: CompressionInput,
    rate: CompressionRate,
    cancel: &AtomicBool,
    progress: impl Fn(usize, usize),
) -> Result<CompactTranscript, CompressionFailure> {
    let meta = compression::verify(dir, cancel)
        .map_err(|detail| CompressionFailure::ModelValidation { detail })?;
    let config: Config = serde_json::from_reader(
        std::fs::File::open(dir.join("config.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if config.hidden_size != 768 || config.max_position_embeddings != 512 {
        return Err("Incompatible compression configuration".into());
    }
    let mut tokenizer =
        tokenizers::Tokenizer::from_file(dir.join("tokenizer.json")).map_err(|e| e.to_string())?;
    tokenizer.with_truncation(None).map_err(|e| e.to_string())?;
    tokenizer.with_padding(None);
    let cls = tokenizer.token_to_id("[CLS]").ok_or("Missing CLS token")?;
    let sep = tokenizer.token_to_id("[SEP]").ok_or("Missing SEP token")?;
    let mut windows = Vec::new();
    let mut line_words = Vec::new();
    for (line_index, line) in input.lines.iter().enumerate() {
        let (_, body) = split_prefix(line);
        line_words.push(words(body, &input.participants));
        let encoded = tokenizer.encode(body, false).map_err(|e| e.to_string())?;
        let offsets = encoded.get_offsets();
        let mut start = 0;
        while start < encoded.len() {
            let mut end = (start + 510).min(encoded.len());
            if end < encoded.len() {
                // Prefer a nearby sentence end, then a whole word boundary. Long words
                // still progress through all pieces and aggregate into one source span.
                if let Some(boundary) = ((start + 255)..end)
                    .rev()
                    .find(|&i| body[..offsets[i].1].ends_with(['.', '!', '?', ';']))
                {
                    end = boundary + 1;
                } else {
                    while end > start + 1 && offsets[end - 1].1 == offsets[end].0 {
                        end -= 1;
                    }
                    if end == start + 1 {
                        end = (start + 510).min(encoded.len());
                    }
                }
            }
            let mut tokens = vec![cls];
            tokens.extend_from_slice(&encoded.get_ids()[start..end]);
            tokens.push(sep);
            windows.push(Window {
                line: line_index,
                ids: tokens,
                offsets: offsets[start..end].to_vec(),
            });
            start = end;
        }
    }
    compression::cancelled(cancel)?;
    let device = Device::new_metal(0).map_err(|e| e.to_string())?;
    // SAFETY: immutable, checksum-verified installation, never rewritten in place.
    let vb = unsafe {
        VarBuilder::from_mmaped_safetensors(&[dir.join(compression::WEIGHTS)], DType::F16, &device)
    }
    .map_err(|e| e.to_string())?;
    let model = BertClassifier::load(vb, &config).map_err(|e| e.to_string())?;
    for (batch_index, batch) in windows.chunks(8).enumerate() {
        compression::cancelled(cancel)?;
        let width = batch.iter().map(|w| w.ids.len()).max().unwrap_or(2);
        let mut ids = vec![config.pad_token_id as u32; batch.len() * width];
        let mut mask = vec![-10000f32; batch.len() * width];
        for (row, w) in batch.iter().enumerate() {
            ids[row * width..row * width + w.ids.len()].copy_from_slice(&w.ids);
            mask[row * width..row * width + w.ids.len()].fill(0.);
        }
        let scores = (|| -> candle_core::Result<Vec<Vec<Vec<f32>>>> {
            let ids = Tensor::from_vec(ids, (batch.len(), width), &device)?;
            let mask = Tensor::from_vec(mask, (batch.len(), 1, 1, width), &device)?
                .to_dtype(DType::F16)?;
            let logits = model.forward(&ids, &mask)?.to_dtype(DType::F32)?;
            candle_nn::ops::softmax_last_dim(&logits)?.to_vec3::<f32>()
        })()
        .map_err(|e| e.to_string())?;
        for (row, w) in batch.iter().enumerate() {
            for (token, &(start, end)) in w.offsets.iter().enumerate() {
                let score = scores[row][token + 1][1];
                if !score.is_finite() {
                    return Err("Nonfinite compression scores".into());
                }
                for word in &mut line_words[w.line] {
                    if start < word.end && end > word.start {
                        word.score += score;
                        word.count += 1;
                    }
                }
            }
        }
        progress(((batch_index + 1) * 8).min(windows.len()), windows.len());
    }
    device.synchronize().map_err(|e| e.to_string())?;
    // No GPU state crosses the return value or the later generative summary call.
    drop(model);
    drop(tokenizer);
    drop(device);
    let mut ranked: Vec<(usize, usize, f32)> = line_words
        .iter_mut()
        .enumerate()
        .flat_map(|(line, ws)| {
            ws.iter_mut().enumerate().map(move |(index, w)| {
                if w.count > 0 {
                    w.score /= w.count as f32;
                }
                (line, index, w.score)
            })
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.2.total_cmp(&a.2)
            .then_with(|| a.0.cmp(&b.0))
            .then_with(|| a.1.cmp(&b.1))
    });
    let target = (ranked.len() as f32 * rate.get()).ceil() as usize;
    for &(line, word, _) in ranked.iter().take(target) {
        line_words[line][word].keep = true;
    }
    let text = input
        .lines
        .iter()
        .zip(line_words.iter())
        .map(|(line, ws)| {
            let (prefix, body) = split_prefix(line);
            format!("{prefix}{}", reconstruct(body, ws))
        })
        .collect::<Vec<_>>()
        .join("\n");
    compression::cancelled(cancel)?;
    Ok(CompactTranscript {
        approx_tokens: souffle_schema::estimate_tokens(&text),
        text,
        model: provenance(&meta),
        rate: rate.get(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        is_stale: false,
        source_sha256: input.source_sha256,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn overlapping_compression_install_waiters_observe_same_cancelled_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let service = Arc::new(CompressionService::new(dir.path().join("missing")));
        let attempt = Arc::new(InstallationAttempt {
            cancel: Arc::new(AtomicBool::new(false)),
            result: Mutex::new(None),
            done: tokio::sync::Notify::new(),
            recording_epoch: 0,
        });
        *service.install.lock().unwrap() = Some(attempt.clone());
        let mut first = Box::pin(service.ensure_model(std::sync::Weak::new()));
        let mut second = Box::pin(service.ensure_model(std::sync::Weak::new()));
        assert!(futures_util::poll!(first.as_mut()).is_pending());
        assert!(futures_util::poll!(second.as_mut()).is_pending());
        service.cancel_install();
        *attempt.result.lock().unwrap() = Some(Err("cancelled fixture attempt".into()));
        attempt.done.notify_waiters();
        assert_eq!(first.await, Err("cancelled fixture attempt".into()));
        assert_eq!(second.await, Err("cancelled fixture attempt".into()));
        assert!(Arc::ptr_eq(
            service.install.lock().unwrap().as_ref().unwrap(),
            &attempt
        ));
        assert!(!service.model_dir.exists());
    }
    #[test]
    fn cancelled_compression_ignores_late_progress_and_keeps_completed_jobs_idle() {
        let dir = tempfile::tempdir().unwrap();
        let service = CompressionService::new(dir.path().join("not-installed"));
        let cancelled = service.start_job("cancelled");
        service.cancel("cancelled");
        service.update_job(
            "cancelled",
            &cancelled.generation,
            CompactJobStatus::Compressing {
                completed: 1,
                total: 2,
            },
        );
        assert_eq!(
            service.job("cancelled").unwrap().status,
            CompactJobStatus::Idle
        );
        let active = service.start_job("active");
        let completed = service.start_job("completed");
        service.update_job("completed", &completed.generation, CompactJobStatus::Idle);
        service.recording_started();
        service.update_job(
            "active",
            &active.generation,
            CompactJobStatus::Compressing {
                completed: 1,
                total: 2,
            },
        );
        assert_eq!(
            service.job("active").unwrap().status,
            CompactJobStatus::InterruptedByRecording
        );
        assert_eq!(
            service.job("completed").unwrap().status,
            CompactJobStatus::Idle
        );
    }
    #[test]
    fn rate_and_unicode_protection() {
        assert!(CompressionRate::new(f32::NAN).is_err());
        assert!(CompressionRate::new(0.1).is_err());
        let body = "Élodie Martin n’approuve pas les 42 dépenses, néanmoins.";
        let ws = words(body, &["Élodie Martin".into()]);
        let kept = reconstruct(body, &ws);
        assert_eq!(kept, "Élodie Martin n’approuve pas 42");
        assert_eq!(
            split_prefix("[12:03] Them: Bonjour"),
            ("[12:03] Them: ", "Bonjour")
        );
        assert_eq!(split_prefix("Texte édité"), ("", "Texte édité"));
    }
    #[test]
    #[ignore = "requires pinned cached model; synthetic text only"]
    fn real_cached_model_smoke() {
        let dir = std::env::var_os("SOUFFLE_COMPRESSION_TEST_CACHE")
            .map(PathBuf::from)
            .expect("cache directory");
        if compression::installed(&dir).is_none() {
            let source = std::env::var_os("SOUFFLE_COMPRESSION_TEST_SOURCE")
                .map(PathBuf::from)
                .expect("pinned source snapshot");
            compression::prepare_cached(&source, &dir).unwrap();
        }
        let input=CompressionInput::new(vec!["[0:12] Me: Élodie Martin ne doit pas payer 42 euros pour cette longue discussion sans décision finale.".into(),"Plain edited English sentence about a detailed and rather repetitive meeting plan with seven possible ideas.".into()],vec!["Élodie Martin".into()],&Vec::<String>::new(),false,false);
        let output = compress(
            &dir,
            input,
            CompressionRate::new(0.5).unwrap(),
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
        assert!(output.text.contains("Élodie Martin"));
        assert!(output.text.contains("ne"));
        assert!(output.text.contains("pas"));
        assert!(output.text.contains("42"));
        assert_eq!(output.text.lines().count(), 2);
        assert!(output.text.starts_with("[0:12] Me: "));
        println!("{}", output.text);
    }
}
