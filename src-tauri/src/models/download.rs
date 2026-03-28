use std::path::Path;

use tracing::info;
use crate::engine::ModelArtifactDescriptor;

/// Download status reported to the frontend
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct DownloadProgress {
    pub file: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub status: DownloadStatus,
}

#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStatus {
    Starting,
    Downloading,
    Complete,
    Error(String),
}

/// Check if all required model files exist locally
pub fn model_exists(model_dir: &Path, required_files: &[String]) -> bool {
    if !model_dir.exists() {
        return false;
    }
    if !required_files.iter().all(|file| model_dir.join(file).exists()) {
        return false;
    }

    let config_path = model_dir.join("config.json");
    let Ok(config_str) = std::fs::read_to_string(&config_path) else {
        return true;
    };
    let Ok(config) = serde_json::from_str::<serde_json::Value>(&config_str) else {
        return false;
    };

    ["mimi_name", "tokenizer_name"].iter().all(|field| {
        config[*field]
            .as_str()
            .is_none_or(|file_name| model_dir.join(file_name).exists())
    })
}

/// Download model files from HuggingFace using hf-hub.
/// hf-hub handles caching, resumption, and stores files in its own cache.
/// We then symlink/copy the resolved paths to our model dir for easy access.
pub fn download_model(
    artifact: &ModelArtifactDescriptor,
    model_dir: &Path,
    progress_callback: impl Fn(DownloadProgress),
) -> Result<(), String> {
    std::fs::create_dir_all(model_dir).map_err(|e| format!("Failed to create model dir: {e}"))?;

    let api =
        hf_hub::api::sync::Api::new().map_err(|e| format!("HuggingFace API init failed: {e}"))?;
    let repo = api.model(artifact.repository.clone());

    // First download config.json to discover other file names
    progress_callback(DownloadProgress {
        file: "config.json".into(),
        downloaded_bytes: 0,
        total_bytes: None,
        status: DownloadStatus::Starting,
    });

    let config_path = repo
        .get("config.json")
        .map_err(|e| format!("Failed to download config.json: {e}"))?;

    // Copy to our model dir
    let dest_config = model_dir.join("config.json");
    std::fs::copy(&config_path, &dest_config)
        .map_err(|e| format!("Failed to copy config.json: {e}"))?;

    progress_callback(DownloadProgress {
        file: "config.json".into(),
        downloaded_bytes: 0,
        total_bytes: None,
        status: DownloadStatus::Complete,
    });

    // Parse config to get mimi and tokenizer file names
    let config_str = std::fs::read_to_string(&dest_config)
        .map_err(|e| format!("Failed to read config.json: {e}"))?;
    let config: serde_json::Value =
        serde_json::from_str(&config_str).map_err(|e| format!("Invalid config.json: {e}"))?;

    let mimi_name = config["mimi_name"]
        .as_str()
        .ok_or_else(|| "Missing mimi_name in config".to_string())?;
    let tokenizer_name = config["tokenizer_name"]
        .as_str()
        .ok_or_else(|| "Missing tokenizer_name in config".to_string())?;

    // Download remaining files
    let files_to_download = vec![
        ("model.safetensors", "model.safetensors"),
        (mimi_name, mimi_name),
        (tokenizer_name, tokenizer_name),
    ];

    for (repo_file, local_name) in &files_to_download {
        info!(file = repo_file, "Downloading");
        progress_callback(DownloadProgress {
            file: local_name.to_string(),
            downloaded_bytes: 0,
            total_bytes: None,
            status: DownloadStatus::Downloading,
        });

        let cached_path = repo
            .get(repo_file)
            .map_err(|e| format!("Failed to download {repo_file}: {e}"))?;

        let dest = model_dir.join(local_name);
        // hf-hub caches files; create symlink to avoid doubling disk usage
        if dest.exists() {
            std::fs::remove_file(&dest).ok();
        }

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&cached_path, &dest)
                .or_else(|_| std::fs::copy(&cached_path, &dest).map(|_| ()))
                .map_err(|e| format!("Failed to link {local_name}: {e}"))?;
        }
        #[cfg(not(unix))]
        {
            std::fs::copy(&cached_path, &dest)
                .map_err(|e| format!("Failed to copy {local_name}: {e}"))?;
        }

        progress_callback(DownloadProgress {
            file: local_name.to_string(),
            downloaded_bytes: 0,
            total_bytes: None,
            status: DownloadStatus::Complete,
        });
        info!(file = local_name, "Download complete");
    }

    info!("All model files downloaded");
    Ok(())
}
