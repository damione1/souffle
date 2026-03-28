pub mod download;

use std::path::{Path, PathBuf};

use crate::engine::{TranscriptionProfile, resolve_transcription_artifact};

pub use download::{DownloadProgress, DownloadStatus};

pub fn model_exists(profile: &TranscriptionProfile) -> bool {
    if ensure_model_layout(profile).is_err() {
        return false;
    }

    let Ok(artifact) = resolve_transcription_artifact(profile) else {
        return false;
    };

    download::model_exists(&model_dir(profile), &artifact.required_files)
}

pub fn model_dir(profile: &TranscriptionProfile) -> PathBuf {
    crate::constants::app_data_dir()
        .join("models")
        .join(&profile.engine_id)
        .join(&profile.model_id)
        .join(&profile.backend_id)
}

pub fn download_model(
    profile: &TranscriptionProfile,
    progress_callback: impl Fn(DownloadProgress),
) -> Result<(), String> {
    ensure_model_layout(profile)?;
    let artifact = resolve_transcription_artifact(profile)?;
    let model_dir = model_dir(profile);
    download::download_model(&artifact, &model_dir, progress_callback)
}

fn ensure_model_layout(profile: &TranscriptionProfile) -> Result<(), String> {
    let target_dir = model_dir(profile);
    if target_dir.exists() {
        return Ok(());
    }

    let Some(legacy_dir) = legacy_model_dir(profile) else {
        return Ok(());
    };
    if !legacy_dir.exists() {
        return Ok(());
    }

    if let Some(parent) = target_dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create model parent directory: {e}"))?;
    }

    match std::fs::rename(&legacy_dir, &target_dir) {
        Ok(()) => Ok(()),
        Err(_) => copy_directory_contents(&legacy_dir, &target_dir),
    }
}

fn legacy_model_dir(profile: &TranscriptionProfile) -> Option<PathBuf> {
    if profile.engine_id == crate::engine::KYUTAI_ENGINE_ID
        && profile.model_id == crate::engine::KYUTAI_MODEL_ID
        && profile.backend_id == crate::engine::CANDLE_BACKEND_ID
    {
        return Some(
            crate::constants::app_data_dir()
                .join("models")
                .join(&profile.engine_id)
                .join(&profile.model_id),
        );
    }

    None
}

fn copy_directory_contents(source: &Path, target: &Path) -> Result<(), String> {
    std::fs::create_dir_all(target).map_err(|e| format!("Failed to create target dir: {e}"))?;

    for entry in std::fs::read_dir(source).map_err(|e| format!("Failed to read source dir: {e}"))? {
        let entry = entry.map_err(|e| format!("Failed to read source entry: {e}"))?;
        let entry_type = entry
            .file_type()
            .map_err(|e| format!("Failed to inspect source entry type: {e}"))?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());

        if entry_type.is_dir() {
            copy_directory_contents(&source_path, &target_path)?;
            continue;
        }

        std::fs::copy(&source_path, &target_path)
            .map_err(|e| format!("Failed to copy model file '{}': {e}", source_path.display()))?;
    }

    Ok(())
}
