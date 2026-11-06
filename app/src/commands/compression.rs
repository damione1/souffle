//! Shared standalone and summary compression path.
use crate::{
    models::compression,
    state::AppState,
    summary::compress::{CompactJobStatus, CompressionFailure, CompressionInput, CompressionRate},
    transcript::CompactTranscript,
};
use std::sync::Arc;

#[derive(Clone, Copy)]
enum ModelAdmission {
    AllowInstallation,
    ReadyOnly,
}

pub async fn generate_compact_transcript(
    state: Arc<AppState>,
    id: String,
    rate: Option<f32>,
    regenerate: bool,
) -> Result<CompactTranscript, String> {
    generate_with_admission(
        state,
        id,
        rate,
        regenerate,
        ModelAdmission::AllowInstallation,
    )
    .await
}
pub async fn generate_compact_transcript_ready(
    state: Arc<AppState>,
    id: String,
    rate: Option<f32>,
) -> Result<CompactTranscript, String> {
    generate_with_admission(state, id, rate, false, ModelAdmission::ReadyOnly).await
}

async fn generate_with_admission(
    state: Arc<AppState>,
    id: String,
    rate: Option<f32>,
    regenerate: bool,
    admission: ModelAdmission,
) -> Result<CompactTranscript, String> {
    if super::settings_capture_busy(&state).map_err(|e| e.to_string())? {
        return Err("Recording is active; retry after recording".into());
    }
    let service = state.compression.clone();
    let epoch = service.recording_epoch();
    let request = service.meeting_request(&id);
    let (request_guard, coalesced) = match request.clone().try_lock_owned() {
        Ok(guard) => (guard, false),
        Err(_) => match admission {
            ModelAdmission::AllowInstallation => (request.lock_owned().await, true),
            ModelAdmission::ReadyOnly => {
                return Err("Compression already in progress; summary uses original input".into());
            }
        },
    };
    let _request_guard = request_guard;
    if service.recording_epoch() != epoch {
        return Err("Compression interrupted by recording".into());
    }
    let initial = state.db.load_meeting(&id)?;
    if initial.ended_at.is_none() {
        return Err("Cannot compact an ongoing meeting".into());
    }
    let rate = CompressionRate::new(
        rate.unwrap_or(crate::settings::AppSettings::load(&state.db)?.summary_compression_rate),
    )?;
    if !regenerate || coalesced {
        if let Some(compact) = &initial.compact_transcript {
            if service.compatible(compact, &CompressionInput::from_meeting(&initial), rate) {
                return Ok(compact.clone());
            }
        }
    }
    if let ModelAdmission::ReadyOnly = admission {
        let ready = match service.model_status() {
            compression::ModelStatus::Ready { .. } => {
                compression::installed(&service.model_dir).is_some()
            }
            compression::ModelStatus::NotDownloaded
            | compression::ModelStatus::Downloading { .. }
            | compression::ModelStatus::Preparing
            | compression::ModelStatus::Failed { .. } => false,
        };
        if !ready {
            return Err("Compression model unavailable; summary uses original input".into());
        }
    }
    let job = service.start_job(&id);
    let result: Result<CompactTranscript, String> = async {
        let retry_model = match service.model_status() {
            compression::ModelStatus::Failed { .. } => true,
            compression::ModelStatus::NotDownloaded
            | compression::ModelStatus::Downloading { .. }
            | compression::ModelStatus::Preparing
            | compression::ModelStatus::Ready { .. } => false,
        };
        if compression::installed(&service.model_dir).is_none() || retry_model {
            service.update_job(&id, &job.generation, CompactJobStatus::WaitingForModel);
            match admission {
                ModelAdmission::AllowInstallation => {
                    service.ensure_model(Arc::downgrade(&state)).await?
                }
                ModelAdmission::ReadyOnly => {
                    return Err("Compression model unavailable; summary uses original input".into());
                }
            }
        }
        compression::cancelled(&job.cancel)?;
        service.update_job(&id, &job.generation, CompactJobStatus::Queued);
        let permit = service
            .compute
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| e.to_string())?;
        compression::cancelled(&job.cancel)?;
        if super::settings_capture_busy(&state).map_err(|e| e.to_string())? {
            return Err("Recording is active; retry after recording".into());
        }
        // Take the immutable source only after model/compute admission; no DB lock survives inference.
        let snapshot = state.db.begin_compaction(&id)?;
        let input = snapshot.input.clone();
        let dir = service.model_dir.clone();
        let worker_service = service.clone();
        let worker_id = id.clone();
        let generation = job.generation.clone();
        let cancel = job.cancel.clone();
        service.update_job(
            &id,
            &job.generation,
            CompactJobStatus::Compressing {
                completed: 0,
                total: 0,
            },
        );
        let compact = tokio::task::spawn_blocking(move || {
            crate::summary::compress::compress(&dir, input, rate, &cancel, |completed, total| {
                worker_service.update_job(
                    &worker_id,
                    &generation,
                    CompactJobStatus::Compressing { completed, total },
                )
            })
        })
        .await
        .map_err(|e| e.to_string())?;
        let compact = match compact {
            Ok(compact) => compact,
            Err(CompressionFailure::ModelValidation { detail }) => {
                if !job.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    service.set_model(compression::ModelStatus::Failed {
                        detail: detail.clone(),
                    });
                }
                return Err(detail);
            }
            Err(CompressionFailure::Inference { detail }) => return Err(detail),
        };
        // The blocking scope has dropped every model/tensor/device before releasing admission.
        drop(permit);
        compression::cancelled(&job.cancel)?;
        match state.db.commit_compaction(&id, &snapshot, &compact)? {
            crate::db::compression::CompactCommit::Committed => Ok(compact),
            crate::db::compression::CompactCommit::Superseded => {
                service.update_job(&id, &job.generation, CompactJobStatus::SourceChanged);
                Err("Meeting source changed; generate again".into())
            }
            crate::db::compression::CompactCommit::Deleted => Err("Meeting was deleted".into()),
        }
    }
    .await;
    match &result {
        Ok(_) => service.update_job(&id, &job.generation, CompactJobStatus::Idle),
        Err(error) => {
            if !job.cancel.load(std::sync::atomic::Ordering::Relaxed)
                && match service.job(&id).map(|j| j.status) {
                    Some(CompactJobStatus::SourceChanged) => false,
                    Some(
                        CompactJobStatus::Idle
                        | CompactJobStatus::WaitingForModel
                        | CompactJobStatus::Queued
                        | CompactJobStatus::Compressing { .. }
                        | CompactJobStatus::Failed { .. }
                        | CompactJobStatus::InterruptedByRecording,
                    )
                    | None => true,
                }
            {
                service.update_job(
                    &id,
                    &job.generation,
                    CompactJobStatus::Failed {
                        detail: error.clone(),
                    },
                );
            }
        }
    }
    result
}
