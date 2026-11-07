//! Compression admission and persistence. Commands delegate this lifecycle to the
//! concrete service; no application/DB lock survives a queue wait or inference.
use super::{
    CompactJobStatus, CompressionFailure, CompressionInput, CompressionRate, CompressionService,
};
use crate::{models::compression, state::AppState, transcript::CompactTranscript};
use std::sync::{Arc, atomic::Ordering};

struct GenerationRequest {
    service: Arc<CompressionService>,
    id: String,
    job: super::CompactJob,
    epoch: u64,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}
impl Drop for GenerationRequest {
    fn drop(&mut self) {
        let mut jobs = self.service.jobs.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(job) = jobs
            .get_mut(&self.id)
            .filter(|job| job.generation == self.job.generation)
        {
            match job.status {
                CompactJobStatus::WaitingForModel
                | CompactJobStatus::Queued
                | CompactJobStatus::Compressing { .. } => {
                    job.cancel.store(true, Ordering::Relaxed);
                    job.status = CompactJobStatus::Idle;
                }
                CompactJobStatus::Idle
                | CompactJobStatus::Failed { .. }
                | CompactJobStatus::SourceChanged
                | CompactJobStatus::InterruptedByRecording => {}
            }
        }
    }
}

/// Only queue waits may be abandoned. Submitted Metal work must drain before
/// releasing its compute permit, and checks the same flag between batches.
async fn cancellable<T>(
    service: &CompressionService,
    epoch: u64,
    cancel: &std::sync::atomic::AtomicBool,
    future: impl std::future::Future<Output = Result<T, CompressionError>>,
) -> Result<T, CompressionError> {
    tokio::select! {
        biased;
        error = async {
            loop {
                if let Err(error) = check_cancel(service, epoch, cancel) { return error; }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        } => Err(error),
        result = future => result,
    }
}

/// Why local compaction could not complete. Keep these causes typed until an
/// existing CLI, generic summary, or native presentation boundary formats them.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CompressionError {
    #[error("Compression cancelled")]
    Cancelled,
    #[error("Compression interrupted by recording; retry after recording")]
    InterruptedByRecording,
    #[error("Recording is active; retry after recording")]
    CaptureBusy,
    #[error("Runtime state is busy; retry shortly")]
    RuntimeBusy,
    #[error("Meeting source changed; generate again")]
    SourceChanged,
    #[error("Meeting was deleted")]
    Deleted,
    #[error("Cannot compact an ongoing meeting")]
    Ongoing,
    #[error("Transcript has no text")]
    EmptyTranscript,
    #[error("Compression rate must be finite and between 0.3 and 0.9")]
    InvalidRate,
    #[error("Compression model unavailable; summary uses original input")]
    ModelUnavailable,
    #[error("Compression already in progress; summary uses original input")]
    AlreadyRunning,
    #[error("Compression model is busy; retry after the current operation")]
    ModelBusy,
    #[error("Compression cache removal failed: {detail}")]
    ModelRemoval { detail: String },
    #[error("Compression model preparation failed: {detail}")]
    ModelPreparation { detail: String },
    #[error("Compression model validation failed: {detail}")]
    ModelValidation { detail: String },
    #[error("Compression inference failed: {detail}")]
    Inference { detail: String },
    #[error("Compression database operation failed: {detail}")]
    Database { detail: String },
    #[error("Compression worker failed: {detail}")]
    Runtime { detail: String },
}

/// Summary requests can use an installed model only. Explicit generation may
/// install/repair it; the policy cannot accidentally start a summary download.
#[derive(Debug, Clone, Copy)]
pub enum CompressionAdmission {
    InstallIfNeeded,
    Native { intent: super::NativeInstallIntent },
    ReadyOnly,
}

/// A nonblocking capture observation. Lock contention is unknown, never evidence
/// of recording, and must not cancel an active compression/model installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionCapture {
    Idle,
    Capturing,
    Unknown,
}

/// Observe the canonical capture reservation and machine without waiting for
/// either lock. The capture gate is checked first, matching its normal order.
pub fn observe_capture(state: &AppState) -> CompressionCapture {
    observe_capture_parts(&state.capture_start, &state.machine)
}
fn observe_capture_parts(
    gate: &crate::audio::start_gate::CaptureStartGate,
    machine: &std::sync::Mutex<crate::state_machine::AppStateMachine>,
) -> CompressionCapture {
    gate.try_with_start_boundary(|starting| {
        if starting {
            return CompressionCapture::Capturing;
        }
        match machine.try_lock() {
            Ok(machine) => {
                if machine.is_recording() {
                    CompressionCapture::Capturing
                } else {
                    CompressionCapture::Idle
                }
            }
            Err(std::sync::TryLockError::WouldBlock | std::sync::TryLockError::Poisoned(_)) => {
                CompressionCapture::Unknown
            }
        }
    })
    .unwrap_or(CompressionCapture::Unknown)
}

/// Retry transient observation contention for at most half a second without
/// holding locks. Confirmed capture and the start epoch always retain priority.
pub(super) async fn wait_capture_idle(
    state: &AppState,
    service: &CompressionService,
    epoch: u64,
) -> Result<(), CompressionError> {
    for attempt in 0..=25 {
        if service.recording_epoch() != epoch {
            return Err(CompressionError::InterruptedByRecording);
        }
        match observe_capture(state) {
            CompressionCapture::Idle => return Ok(()),
            CompressionCapture::Capturing => return Err(CompressionError::CaptureBusy),
            CompressionCapture::Unknown => {
                if attempt < 25 {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            }
        }
    }
    Err(CompressionError::RuntimeBusy)
}
fn check_cancel(
    service: &CompressionService,
    epoch: u64,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), CompressionError> {
    if service.recording_epoch() != epoch {
        Err(CompressionError::InterruptedByRecording)
    } else if cancel.load(Ordering::Relaxed) {
        Err(CompressionError::Cancelled)
    } else {
        Ok(())
    }
}
fn database(detail: String) -> CompressionError {
    CompressionError::Database { detail }
}

/// The two independent user preferences have three valid preparation outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SummaryCompressionUse {
    Disabled,
    CompactOnly,
    CompactAndMap,
}
fn summary_compression_use(
    settings: &crate::settings::AppSettings,
    needs_map: bool,
) -> SummaryCompressionUse {
    if settings.summary_compression_enabled && needs_map {
        SummaryCompressionUse::CompactAndMap
    } else if settings.compact_transcript_enabled {
        SummaryCompressionUse::CompactOnly
    } else {
        SummaryCompressionUse::Disabled
    }
}

impl CompressionService {
    /// Prepare optional compressed map input below the command layer. The original
    /// source still selects stuff/map; the alongside preference persists a compact
    /// even for short summaries, and missing/failed compression always falls back.
    pub async fn prepare_summary_map(
        self: &Arc<Self>,
        state: Arc<AppState>,
        meeting: &crate::transcript::MeetingTranscript,
        source: &CompressionInput,
        text: &str,
        model: &str,
        settings: &crate::settings::AppSettings,
    ) -> Result<Option<crate::summary::CompressedMapInput>, crate::summary::SummaryCommandError>
    {
        let needs_map = crate::summary::needs_map(text, model)
            .map_err(|detail| crate::summary::SummaryCommandError::Provider { detail })?;
        let usage = summary_compression_use(settings, needs_map);
        match usage {
            SummaryCompressionUse::Disabled => return Ok(None),
            SummaryCompressionUse::CompactOnly | SummaryCompressionUse::CompactAndMap => {}
        }
        let fresh = meeting.compact_transcript.as_ref().is_some_and(|compact| {
            CompressionRate::new(settings.summary_compression_rate)
                .is_ok_and(|rate| self.compatible(compact, source, rate))
        });
        if !fresh && compression::installed(&self.model_dir).is_none() {
            tracing::warn!("Compression model unavailable; summarizing original transcript");
            return Ok(None);
        }
        let compact = match self
            .generate(
                state,
                meeting.id.clone(),
                Some(settings.summary_compression_rate),
                false,
                CompressionAdmission::ReadyOnly,
            )
            .await
        {
            Ok(compact) => compact,
            Err(error) => {
                tracing::warn!("Compression unavailable; summarizing original transcript: {error}");
                return Ok(None);
            }
        };
        if compact.source_sha256 != source.source_sha256 {
            tracing::warn!(
                "Meeting changed during compression; summary keeps its original snapshot"
            );
            return Ok(None);
        }
        match usage {
            SummaryCompressionUse::Disabled | SummaryCompressionUse::CompactOnly => Ok(None),
            SummaryCompressionUse::CompactAndMap => Ok(Some(crate::summary::CompressedMapInput {
                turns: source
                    .segmented
                    .then(|| compact.text.split('\n').map(str::to_owned).collect()),
                text: compact.text,
            })),
        }
    }

    /// Generate/reuse a compact through one admitted worker, persist it with a
    /// source/generation CAS, and release all model resources before returning.
    pub fn generate(
        self: &Arc<Self>,
        state: Arc<AppState>,
        id: String,
        rate: Option<f32>,
        regenerate: bool,
        admission: CompressionAdmission,
    ) -> impl std::future::Future<Output = Result<CompactTranscript, CompressionError>>
    + Send
    + 'static
    + use<> {
        // Reservation is synchronous: Cancel owns this identity even before its
        // future is first polled. Duplicate submissions never wait invisibly.
        let lifecycle = self.install.lock().unwrap_or_else(|p| p.into_inner());
        let request = if self.model_status() == compression::ModelStatus::Deleting {
            Err(CompressionError::ModelBusy)
        } else {
            self.meeting_request(&id)
                .try_lock_owned()
                .map_err(|_| CompressionError::AlreadyRunning)
                .map(|guard| GenerationRequest {
                    service: self.clone(),
                    id: id.clone(),
                    job: self.start_job(&id),
                    epoch: self.recording_epoch(),
                    _guard: guard,
                })
        };
        drop(lifecycle);
        let service = self.clone();
        async move {
            let request = request?;
            service
                .generate_reserved(state, id, rate, regenerate, admission, &request)
                .await
        }
    }

    async fn generate_reserved(
        self: &Arc<Self>,
        state: Arc<AppState>,
        id: String,
        rate: Option<f32>,
        regenerate: bool,
        admission: CompressionAdmission,
        request: &GenerationRequest,
    ) -> Result<CompactTranscript, CompressionError> {
        let job = &request.job;
        let epoch = request.epoch;
        let result = async {
            cancellable(
                self,
                epoch,
                &job.cancel,
                wait_capture_idle(&state, self, epoch),
            )
            .await?;
            if self.recording_epoch() != epoch {
                return Err(CompressionError::InterruptedByRecording);
            }
            if !state.db.meeting_exists(&id).map_err(database)? {
                return Err(CompressionError::Deleted);
            }
            let initial = state.db.load_meeting(&id).map_err(database)?;
            if initial.ended_at.is_none() {
                return Err(CompressionError::Ongoing);
            }
            let input = CompressionInput::from_meeting(&initial);
            // Empty source is rejected before any network, cache preparation or queue.
            if input.lines.iter().all(|line| line.trim().is_empty()) {
                return Err(CompressionError::EmptyTranscript);
            }
            let rate = CompressionRate::new(match rate {
                Some(rate) => rate,
                None => {
                    crate::settings::AppSettings::load(&state.db)
                        .map_err(database)?
                        .summary_compression_rate
                }
            })?;
            if !regenerate
                && let Some(compact) = &initial.compact_transcript
                && self.compatible(compact, &input, rate)
            {
                return Ok(compact.clone());
            }
            if let CompressionAdmission::ReadyOnly = admission {
                let ready = match self.model_status() {
                    compression::ModelStatus::Ready { .. } => {
                        compression::installed(&self.model_dir).is_some()
                    }
                    compression::ModelStatus::NotDownloaded
                    | compression::ModelStatus::Queued
                    | compression::ModelStatus::Deleting
                    | compression::ModelStatus::Downloading { .. }
                    | compression::ModelStatus::Preparing
                    | compression::ModelStatus::Failed { .. } => false,
                };
                if !ready {
                    return Err(CompressionError::ModelUnavailable);
                }
            }
            let retry = match self.model_status() {
                compression::ModelStatus::Failed { .. } => true,
                compression::ModelStatus::NotDownloaded
                | compression::ModelStatus::Queued
                | compression::ModelStatus::Deleting
                | compression::ModelStatus::Downloading { .. }
                | compression::ModelStatus::Preparing
                | compression::ModelStatus::Ready { .. } => false,
            };
            if compression::installed(&self.model_dir).is_none() || retry {
                self.update_job(&id, &job.generation, CompactJobStatus::WaitingForModel);
                match admission {
                    CompressionAdmission::InstallIfNeeded => {
                        cancellable(
                            self,
                            epoch,
                            &job.cancel,
                            self.ensure_model(Arc::downgrade(&state)),
                        )
                        .await?
                    }
                    CompressionAdmission::Native { intent } => {
                        cancellable(
                            self,
                            epoch,
                            &job.cancel,
                            self.ensure_model_native(Arc::downgrade(&state), intent),
                        )
                        .await?
                    }
                    CompressionAdmission::ReadyOnly => {
                        return Err(CompressionError::ModelUnavailable);
                    }
                }
            }
            check_cancel(self, epoch, &job.cancel)?;
            self.update_job(&id, &job.generation, CompactJobStatus::Queued);
            let permit = cancellable(self, epoch, &job.cancel, async {
                self.compute.clone().acquire_owned().await.map_err(|error| {
                    CompressionError::Runtime {
                        detail: error.to_string(),
                    }
                })
            })
            .await?;
            check_cancel(self, epoch, &job.cancel)?;
            wait_capture_idle(&state, self, epoch).await?;
            let snapshot = state.db.begin_compaction(&id)?;
            let input = snapshot.input.clone();
            let dir = self.model_dir.clone();
            let worker = self.clone();
            let worker_id = id.clone();
            let generation = job.generation.clone();
            let cancel = job.cancel.clone();
            self.update_job(
                &id,
                &job.generation,
                CompactJobStatus::Compressing {
                    completed: 0,
                    total: 0,
                },
            );
            let compact = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                super::compress(&dir, input, rate, &cancel, |completed, total| {
                    worker.update_job(
                        &worker_id,
                        &generation,
                        CompactJobStatus::Compressing { completed, total },
                    )
                })
            })
            .await
            .map_err(|error| CompressionError::Runtime {
                detail: error.to_string(),
            })?;
            check_cancel(self, epoch, &job.cancel)?;
            let compact = match compact {
                Ok(compact) => compact,
                Err(CompressionFailure::ModelValidation { detail }) => {
                    self.set_model(compression::ModelStatus::Failed {
                        detail: detail.clone(),
                    });
                    return Err(CompressionError::ModelValidation { detail });
                }
                Err(CompressionFailure::Inference { detail }) => {
                    return Err(CompressionError::Inference { detail });
                }
            };
            // The blocking scope has dropped its device, mappings, tokenizer and tensors.

            check_cancel(self, epoch, &job.cancel)?;
            match state
                .db
                .commit_compaction(&id, &snapshot, &compact)
                .map_err(database)?
            {
                crate::db::compression::CompactCommit::Committed => Ok(compact),
                crate::db::compression::CompactCommit::Superseded => {
                    Err(CompressionError::SourceChanged)
                }
                crate::db::compression::CompactCommit::Deleted => Err(CompressionError::Deleted),
            }
        }
        .await;
        let status = match &result {
            Ok(_) => CompactJobStatus::Idle,
            Err(error) => match error {
                CompressionError::Cancelled => CompactJobStatus::Idle,
                CompressionError::InterruptedByRecording | CompressionError::CaptureBusy => {
                    CompactJobStatus::InterruptedByRecording
                }
                CompressionError::SourceChanged => CompactJobStatus::SourceChanged,
                CompressionError::RuntimeBusy
                | CompressionError::Deleted
                | CompressionError::Ongoing
                | CompressionError::EmptyTranscript
                | CompressionError::InvalidRate
                | CompressionError::ModelUnavailable
                | CompressionError::AlreadyRunning
                | CompressionError::ModelBusy
                | CompressionError::ModelRemoval { .. }
                | CompressionError::ModelPreparation { .. }
                | CompressionError::ModelValidation { .. }
                | CompressionError::Inference { .. }
                | CompressionError::Database { .. }
                | CompressionError::Runtime { .. } => CompactJobStatus::Failed {
                    error: error.clone(),
                },
            },
        };
        if let Some(current) = self
            .jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(&id)
            .filter(|current| current.generation == job.generation)
        {
            current.status = status;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compression_summary_policy_preserves_original_short_input_and_independent_compact() {
        let mut settings = crate::settings::AppSettings::default();
        assert_eq!(
            summary_compression_use(&settings, true),
            SummaryCompressionUse::Disabled
        );
        settings.summary_compression_enabled = true;
        assert_eq!(
            summary_compression_use(&settings, false),
            SummaryCompressionUse::Disabled
        );
        assert_eq!(
            summary_compression_use(&settings, true),
            SummaryCompressionUse::CompactAndMap
        );
        settings.compact_transcript_enabled = true;
        assert_eq!(
            summary_compression_use(&settings, false),
            SummaryCompressionUse::CompactOnly
        );
        settings.summary_compression_enabled = false;
        assert_eq!(
            summary_compression_use(&settings, true),
            SummaryCompressionUse::CompactOnly
        );
    }

    fn fixture() -> (Arc<AppState>, tempfile::TempDir) {
        let (db, dir) = crate::test_helpers::fixtures::test_db();
        let db = Arc::new(db);
        let (_audio, rx) = crossbeam_channel::unbounded();
        let actor = crate::pipeline::EngineActorHandle::spawn(
            rx,
            Arc::new(std::sync::atomic::AtomicU64::new(0)),
            Arc::new(std::sync::Mutex::new(None)),
            Box::new(|_| Err("No transcription in compression regression".into())),
        )
        .unwrap();
        let (tx, _commands) = crossbeam_channel::unbounded();
        let mut state = AppState::new(
            tx,
            Arc::new(actor),
            db,
            Arc::new(std::sync::atomic::AtomicU32::new(0)),
        );
        state.compression = Arc::new(CompressionService::new(dir.path().join("missing-model")));
        (Arc::new(state), dir)
    }
    #[tokio::test]
    async fn compression_reserves_cancel_identity_before_poll_and_rejects_duplicate() {
        let (state, _dir) = fixture();
        state
            .db
            .save_meeting(&crate::test_helpers::fixtures::sample_meeting("cancel"))
            .unwrap();
        let first = state.compression.generate(
            state.clone(),
            "cancel".into(),
            None,
            true,
            CompressionAdmission::InstallIfNeeded,
        );
        let generation = state.compression.job("cancel").unwrap().generation;
        let duplicate = state.compression.generate(
            state.clone(),
            "cancel".into(),
            None,
            true,
            CompressionAdmission::InstallIfNeeded,
        );
        assert_eq!(duplicate.await, Err(CompressionError::AlreadyRunning));
        assert_eq!(
            state.compression.job("cancel").unwrap().generation,
            generation
        );
        state.compression.cancel("cancel");
        assert_eq!(
            state.compression.job("cancel").unwrap().status,
            CompactJobStatus::Queued
        );
        assert_eq!(first.await, Err(CompressionError::Cancelled));
        assert_eq!(
            state.compression.job("cancel").unwrap().status,
            CompactJobStatus::Idle
        );
        assert!(!state.compression.model_dir.exists());
        assert!(
            state
                .db
                .load_compact_transcript("cancel")
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn compression_cancel_abandons_compute_and_shared_install_waits() {
        let (state, _dir) = fixture();
        let service = &state.compression;
        let job = service.start_job("wait");
        let permit = service.compute.clone().acquire_owned().await.unwrap();
        let mut queued = Box::pin(cancellable(service, 0, &job.cancel, async {
            service
                .compute
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| CompressionError::RuntimeBusy)
        }));
        assert!(futures_util::poll!(queued.as_mut()).is_pending());
        service.cancel("wait");
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(1), queued)
                .await
                .unwrap(),
            Err(CompressionError::Cancelled)
        ));
        drop(permit);
        let job = service.start_job("install-wait");
        let mut pending = Box::pin(cancellable(
            service,
            0,
            &job.cancel,
            std::future::pending::<Result<(), CompressionError>>(),
        ));
        assert!(futures_util::poll!(pending.as_mut()).is_pending());
        service.cancel("install-wait");
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), pending)
                .await
                .unwrap(),
            Err(CompressionError::Cancelled)
        );
    }

    #[tokio::test]
    async fn compression_native_delayed_intent_cannot_install_after_opt_out() {
        let (state, _dir) = fixture();
        let mut settings = crate::settings::AppSettings {
            meeting_summaries_enabled: true,
            ..Default::default()
        };
        settings.save(&state.db).unwrap();
        let intent = state.compression.native_install_intent();
        settings.meeting_summaries_enabled = false;
        settings.save(&state.db).unwrap();
        state.compression.cancel_install_if_disabled(&state);
        assert_eq!(
            state
                .compression
                .ensure_model_native(Arc::downgrade(&state), intent)
                .await,
            Err(CompressionError::Cancelled)
        );
        settings.meeting_summaries_enabled = true;
        settings.save(&state.db).unwrap();
        assert_eq!(
            state
                .compression
                .ensure_model_native(Arc::downgrade(&state), intent)
                .await,
            Err(CompressionError::Cancelled)
        );
        assert!(!state.compression.model_dir.exists());
    }

    #[tokio::test]
    async fn compression_install_capture_busy_is_terminal_and_observable() {
        let (state, _dir) = fixture();
        state.capture_start.begin(1);
        assert_eq!(
            state.compression.ensure_model(Arc::downgrade(&state)).await,
            Err(CompressionError::CaptureBusy)
        );
        assert_eq!(
            state.compression.model_error(),
            Some(CompressionError::CaptureBusy)
        );
        assert_eq!(
            state.compression.model_status(),
            compression::ModelStatus::NotDownloaded
        );
        assert!(!state.compression.model_dir.exists());
    }

    #[tokio::test]
    async fn compression_empty_and_edited_source_admission_stays_typed_without_download() {
        let (state, _dir) = fixture();
        let mut meeting = crate::test_helpers::fixtures::sample_meeting("empty");
        meeting.segments.clear();
        state.db.save_meeting(&meeting).unwrap();
        assert_eq!(
            state
                .compression
                .generate(
                    state.clone(),
                    "empty".into(),
                    Some(0.5),
                    false,
                    CompressionAdmission::InstallIfNeeded
                )
                .await,
            Err(CompressionError::EmptyTranscript)
        );
        assert!(!state.compression.model_dir.exists());
        assert!(matches!(
            state.compression.job("empty").unwrap().status,
            CompactJobStatus::Failed {
                error: CompressionError::EmptyTranscript
            }
        ));
        state
            .db
            .save_edited_transcript("empty", Some("Plain edited source remains valid"))
            .unwrap();
        assert_eq!(
            state
                .compression
                .generate(
                    state.clone(),
                    "empty".into(),
                    Some(0.5),
                    false,
                    CompressionAdmission::ReadyOnly
                )
                .await,
            Err(CompressionError::ModelUnavailable)
        );
        assert_eq!(
            state
                .compression
                .generate(
                    state.clone(),
                    "empty".into(),
                    Some(f32::NAN),
                    false,
                    CompressionAdmission::InstallIfNeeded
                )
                .await,
            Err(CompressionError::InvalidRate)
        );
        assert_eq!(
            state
                .compression
                .generate(
                    state.clone(),
                    "deleted".into(),
                    None,
                    false,
                    CompressionAdmission::InstallIfNeeded
                )
                .await,
            Err(CompressionError::Deleted)
        );
        meeting.ended_at = None;
        state.db.save_meeting(&meeting).unwrap();
        assert_eq!(
            state
                .compression
                .generate(
                    state.clone(),
                    "empty".into(),
                    None,
                    false,
                    CompressionAdmission::InstallIfNeeded
                )
                .await,
            Err(CompressionError::Ongoing)
        );
        assert!(!state.compression.model_dir.exists());
    }
    #[tokio::test]
    async fn compression_admission_retries_unknown_then_observes_confirmed_capture() {
        let (state, _dir) = fixture();
        let (worker_tx, worker_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker_state = state.clone();
        let worker = std::thread::spawn(move || {
            let _guard = worker_state.machine.lock().unwrap();
            worker_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        worker_rx.recv().unwrap();
        assert_eq!(observe_capture(&state), CompressionCapture::Unknown);
        let epoch = state.compression.recording_epoch();
        let release = tokio::spawn(async move {
            tokio::task::yield_now().await;
            release_tx.send(()).unwrap();
        });
        assert_eq!(
            wait_capture_idle(&state, &state.compression, epoch).await,
            Ok(())
        );
        release.await.unwrap();
        worker.join().unwrap();
        state.capture_start.begin(1);
        assert_eq!(
            wait_capture_idle(&state, &state.compression, epoch).await,
            Err(CompressionError::CaptureBusy)
        );
        state.compression.recording_started();
        assert_eq!(
            wait_capture_idle(&state, &state.compression, epoch).await,
            Err(CompressionError::InterruptedByRecording)
        );
    }
    #[test]
    fn compression_capture_distinguishes_contention_from_recording() {
        let gate = crate::audio::start_gate::CaptureStartGate::new();
        let machine = std::sync::Mutex::new(crate::state_machine::AppStateMachine::Idle);
        assert_eq!(
            observe_capture_parts(&gate, &machine),
            CompressionCapture::Idle
        );
        let lock = machine.lock().unwrap();
        assert_eq!(
            observe_capture_parts(&gate, &machine),
            CompressionCapture::Unknown
        );
        drop(lock);
        gate.with_start_boundary(|_| {
            assert_eq!(
                observe_capture_parts(&gate, &machine),
                CompressionCapture::Unknown
            )
        });
        gate.begin(1);
        assert_eq!(
            observe_capture_parts(&gate, &machine),
            CompressionCapture::Capturing
        );
    }
}
