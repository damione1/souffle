pub mod download;

use std::path::PathBuf;

use crate::engine::{KYUTAI_ENGINE_ID, KYUTAI_MODEL_ID, TranscriptionProfile};

pub use download::{DownloadProgress, DownloadStatus, KYUTAI_HF_REPO};

pub fn model_exists(profile: &TranscriptionProfile) -> bool {
    download::model_exists(&model_dir(profile))
}

pub fn model_dir(profile: &TranscriptionProfile) -> PathBuf {
    match (profile.engine_id.as_str(), profile.model_id.as_str()) {
        (KYUTAI_ENGINE_ID, KYUTAI_MODEL_ID) => download::default_model_dir(),
        _ => crate::constants::app_data_dir()
            .join("models")
            .join(&profile.engine_id)
            .join(&profile.model_id),
    }
}

pub fn download_model(
    profile: &TranscriptionProfile,
    progress_callback: impl Fn(DownloadProgress),
) -> Result<(), String> {
    let model_dir = model_dir(profile);
    match (profile.engine_id.as_str(), profile.model_id.as_str()) {
        (KYUTAI_ENGINE_ID, KYUTAI_MODEL_ID) => {
            download::download_model(&model_dir, progress_callback)
        }
        _ => Err(format!(
            "No download implementation registered for '{}:{}'",
            profile.engine_id, profile.model_id
        )),
    }
}
