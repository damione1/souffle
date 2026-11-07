//! Installation intent is reserved before spawning. Opt-out and admission use
//! the same lock, so a delayed native action cannot resurrect a hidden download.
use super::{CompressionError, CompressionService, InstallationAttempt, compression};
use crate::{settings::AppSettings, state::AppState};
use compression::ModelStatus;
use std::sync::{Arc, Weak, atomic::Ordering};

#[derive(Debug, Clone, Copy)]
pub struct NativeInstallIntent(u64);

impl CompressionService {
    pub fn native_install_intent(&self) -> NativeInstallIntent {
        NativeInstallIntent(self.native_install_epoch.load(Ordering::SeqCst))
    }

    pub fn model_error(&self) -> Option<CompressionError> {
        self.model_error
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub(super) fn publish_model_error(&self, error: Option<CompressionError>) {
        *self.model_error.lock().unwrap_or_else(|p| p.into_inner()) = error;
    }

    /// Reserve removal before dispatch. Busy operations are rejected, never
    /// cancelled behind the user's back or queued to repopulate a deleted cache.
    pub fn remove_model(
        self: &Arc<Self>,
    ) -> impl std::future::Future<Output = Result<(), CompressionError>> + Send + 'static + use<>
    {
        let reserved = (|| {
            let slot = self.install.lock().unwrap_or_else(|p| p.into_inner());
            if slot.as_ref().is_some_and(|attempt| {
                attempt
                    .result
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .is_none()
            }) || self.model_status() == ModelStatus::Deleting
            {
                return Err(CompressionError::ModelBusy);
            }
            let jobs = self.jobs.lock().unwrap_or_else(|p| p.into_inner());
            if jobs.values().any(|job| match job.status {
                super::CompactJobStatus::WaitingForModel
                | super::CompactJobStatus::Queued
                | super::CompactJobStatus::Compressing { .. } => true,
                super::CompactJobStatus::Idle
                | super::CompactJobStatus::Failed { .. }
                | super::CompactJobStatus::SourceChanged
                | super::CompactJobStatus::InterruptedByRecording => false,
            }) {
                return Err(CompressionError::ModelBusy);
            }
            // Recording can mark a job interrupted before its future drains.
            // The owned request guard, not the presentation phase, proves it is gone.
            if self
                .requests
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .values()
                .filter_map(Weak::upgrade)
                .any(|request| request.try_lock().is_err())
            {
                return Err(CompressionError::ModelBusy);
            }
            let permit = self
                .compute
                .clone()
                .try_acquire_owned()
                .map_err(|_| CompressionError::ModelBusy)?;
            self.native_install_epoch.fetch_add(1, Ordering::SeqCst);
            self.set_model(ModelStatus::Deleting);
            self.publish_model_error(None);
            let worker = self.clone();
            // The shared worker owns terminal publication even if a caller disappears.
            Ok(crate::async_runtime::spawn(async move {
                let dir = worker.model_dir.clone();
                let result = tokio::task::spawn_blocking(move || compression::remove(&dir))
                    .await
                    .map_err(|e| CompressionError::Runtime {
                        detail: e.to_string(),
                    })
                    .and_then(|result| {
                        result.map_err(|error| match error {
                            compression::RemovalError::Busy => CompressionError::ModelBusy,
                            compression::RemovalError::Io(error) => {
                                CompressionError::ModelRemoval {
                                    detail: error.to_string(),
                                }
                            }
                        })
                    });
                worker.set_model(
                    compression::installed(&worker.model_dir)
                        .map(|m| ModelStatus::Ready { bytes: m.bytes })
                        .unwrap_or(ModelStatus::NotDownloaded),
                );
                worker.publish_model_error(result.as_ref().err().cloned());
                drop(permit);
                result
            }))
        })();
        if let Err(error) = &reserved {
            self.publish_model_error(Some(error.clone()));
        }
        async move {
            reserved?.await.map_err(|e| CompressionError::Runtime {
                detail: e.to_string(),
            })?
        }
    }

    /// Called after a settings commit. Recheck canonical state under admission's
    /// lock so a late disable response cannot cancel a newer enabling action.
    pub fn cancel_install_if_disabled(&self, state: &AppState) {
        let slot = self.install.lock().unwrap_or_else(|p| p.into_inner());
        let disabled = AppSettings::load_read_only(&state.db)
            .map(|settings| !settings.meeting_summaries_enabled)
            .unwrap_or(true);
        if disabled {
            self.native_install_epoch.fetch_add(1, Ordering::SeqCst);
            if let Some(attempt) = slot.as_ref() {
                attempt.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    /// Explicit headless intent is independent of the UI experimental switch.
    pub fn ensure_model(
        self: &Arc<Self>,
        state: Weak<AppState>,
    ) -> impl std::future::Future<Output = Result<(), CompressionError>> + Send + 'static + use<>
    {
        self.ensure_model_with_intent(state, None)
    }

    pub fn ensure_model_native(
        self: &Arc<Self>,
        state: Weak<AppState>,
        intent: NativeInstallIntent,
    ) -> impl std::future::Future<Output = Result<(), CompressionError>> + Send + 'static + use<>
    {
        self.ensure_model_with_intent(state, Some(intent))
    }

    fn ensure_model_with_intent(
        self: &Arc<Self>,
        state: Weak<AppState>,
        intent: Option<NativeInstallIntent>,
    ) -> impl std::future::Future<Output = Result<(), CompressionError>> + Send + 'static + use<>
    {
        let reserved = (|| {
            let mut slot = self.install.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(intent) = intent {
                let live = state.upgrade().ok_or(CompressionError::Cancelled)?;
                let enabled = AppSettings::load_read_only(&live.db)
                    .map_err(|detail| CompressionError::Database { detail })?
                    .meeting_summaries_enabled;
                if !enabled || intent.0 != self.native_install_epoch.load(Ordering::SeqCst) {
                    return Err(CompressionError::Cancelled);
                }
            }
            match self.model_status() {
                ModelStatus::Deleting => return Err(CompressionError::ModelBusy),
                ModelStatus::Ready { .. } if compression::installed(&self.model_dir).is_some() => {
                    return Ok(None);
                }
                ModelStatus::Ready { .. }
                | ModelStatus::NotDownloaded
                | ModelStatus::Queued
                | ModelStatus::Downloading { .. }
                | ModelStatus::Preparing
                | ModelStatus::Failed { .. } => {}
            }
            if let Some(attempt) = slot.as_ref().filter(|attempt| {
                attempt
                    .result
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .is_none()
            }) {
                return Ok(Some(attempt.clone()));
            }
            let attempt = Arc::new(InstallationAttempt {
                cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                result: std::sync::Mutex::new(None),
                done: tokio::sync::Notify::new(),
                recording_epoch: self.recording_epoch(),
            });
            *slot = Some(attempt.clone());
            self.set_model(ModelStatus::Queued);
            self.publish_model_error(None);
            let worker = self.clone();
            let worker_attempt = attempt.clone();
            // Start while the reservation is authoritative; dropping any one
            // waiter must not strand the shared attempt in Queued forever.
            crate::async_runtime::spawn(async move {
                let result = worker.run_install(state, &worker_attempt).await;
                *worker_attempt
                    .result
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = Some(result);
                worker_attempt.done.notify_waiters();
            });
            Ok(Some(attempt))
        })();
        if let Err(error) = &reserved
            && *error != CompressionError::Cancelled
        {
            self.publish_model_error(Some(error.clone()));
        }
        async move {
            let Some(attempt) = reserved? else {
                return Ok(());
            };
            loop {
                let notified = attempt.done.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let result = attempt
                    .result
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                if let Some(result) = result {
                    return result;
                }
                notified.await;
            }
        }
    }

    async fn run_install(
        self: &Arc<Self>,
        state: Weak<AppState>,
        attempt: &InstallationAttempt,
    ) -> Result<(), CompressionError> {
        let result = async {
            if attempt.cancel.load(Ordering::Relaxed) {
                return Err(CompressionError::Cancelled);
            }
            let state = state.upgrade().ok_or(CompressionError::Cancelled)?;
            super::orchestration::wait_capture_idle(&state, self, attempt.recording_epoch).await?;
            if attempt.cancel.load(Ordering::Relaxed) {
                return Err(CompressionError::Cancelled);
            }
            let service = self.clone();
            let cancel = attempt.cancel.clone();
            let weak = Arc::downgrade(&state);
            let outcome = tokio::task::spawn_blocking(move || {
                compression::install(&service.model_dir, &cancel, |status| {
                    if let Some(state) = weak.upgrade() {
                        match super::observe_capture(&state) {
                            super::CompressionCapture::Capturing => {
                                cancel.store(true, Ordering::Relaxed)
                            }
                            super::CompressionCapture::Idle
                            | super::CompressionCapture::Unknown => {}
                        }
                    } else {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    service.set_model(status);
                })
            })
            .await
            .map_err(|error| CompressionError::Runtime {
                detail: error.to_string(),
            })?;
            outcome
                .map(|_| ())
                .map_err(|detail| CompressionError::ModelPreparation { detail })
        }
        .await;
        let result = if attempt.cancel.load(Ordering::Relaxed) {
            Err(if self.recording_epoch() != attempt.recording_epoch {
                CompressionError::InterruptedByRecording
            } else {
                CompressionError::Cancelled
            })
        } else {
            result
        };
        self.finish_install_result(&result);
        result
    }

    /// Every admission and worker exit produces a terminal, observable state.
    fn finish_install_result(&self, result: &Result<(), CompressionError>) {
        let existing = || {
            compression::installed(&self.model_dir)
                .map(|meta| ModelStatus::Ready { bytes: meta.bytes })
                .unwrap_or(ModelStatus::NotDownloaded)
        };
        match result {
            Ok(()) => {
                self.set_model(existing());
                self.publish_model_error(None);
            }
            Err(error) => {
                let status = match error {
                    CompressionError::Cancelled
                    | CompressionError::InterruptedByRecording
                    | CompressionError::CaptureBusy
                    | CompressionError::RuntimeBusy => existing(),
                    CompressionError::SourceChanged
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
                    | CompressionError::Runtime { .. } => ModelStatus::Failed {
                        detail: error.to_string(),
                    },
                };
                self.set_model(status);
                self.publish_model_error(if *error == CompressionError::Cancelled {
                    None
                } else {
                    Some(error.clone())
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn compression_removal_rejects_pending_work_and_preserves_saved_preferences() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("revision");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("fake"), b"fake weights").unwrap();
        let service = Arc::new(CompressionService::new(dir.clone()));
        let attempt = Arc::new(InstallationAttempt {
            cancel: Arc::new(AtomicBool::new(false)),
            result: std::sync::Mutex::new(None),
            done: tokio::sync::Notify::new(),
            recording_epoch: 0,
        });
        *service.install.lock().unwrap() = Some(attempt.clone());
        assert_eq!(
            service.remove_model().await,
            Err(CompressionError::ModelBusy)
        );
        assert!(!attempt.cancel.load(Ordering::Relaxed));
        *attempt.result.lock().unwrap() = Some(Err(CompressionError::Cancelled));
        let job = service.start_job("active");
        assert_eq!(
            service.remove_model().await,
            Err(CompressionError::ModelBusy)
        );
        service.update_job(
            "active",
            &job.generation,
            super::super::CompactJobStatus::Idle,
        );
        let permit = service.compute.clone().acquire_owned().await.unwrap();
        assert_eq!(
            service.remove_model().await,
            Err(CompressionError::ModelBusy)
        );
        drop(permit);
        service.remove_model().await.unwrap();
        assert!(!dir.exists());
        assert_eq!(service.model_status(), ModelStatus::NotDownloaded);
        assert_eq!(service.model_error(), None);
        // Service removal never receives a DB handle: derived meeting data and
        // durable user preferences are structurally outside its write scope.
    }

    #[test]
    fn compression_worker_failure_is_terminal_and_visible() {
        let temp = tempfile::tempdir().unwrap();
        let service = CompressionService::new(temp.path().join("missing"));
        service.set_model(ModelStatus::Preparing);
        let error = CompressionError::Runtime {
            detail: "stub worker panic".into(),
        };
        service.finish_install_result(&Err(error.clone()));
        assert!(matches!(service.model_status(), ModelStatus::Failed { .. }));
        assert_eq!(service.model_error(), Some(error));
        service.finish_install_result(&Err(CompressionError::Cancelled));
        assert_eq!(service.model_status(), ModelStatus::NotDownloaded);
        assert_eq!(service.model_error(), None);
    }
}
