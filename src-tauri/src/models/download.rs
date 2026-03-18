use std::path::{Path, PathBuf};

use tracing::info;

/// HuggingFace repo for the Candle-compatible Kyutai STT model
pub const KYUTAI_HF_REPO: &str = "kyutai/stt-1b-en_fr-candle";

/// Download status reported to the frontend
#[derive(Debug, Clone, serde::Serialize)]
pub struct DownloadProgress {
    pub file: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub status: DownloadStatus,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStatus {
    Starting,
    Downloading,
    Complete,
    Error(String),
}

/// Check if all required model files exist locally
pub fn model_exists(model_dir: &Path) -> bool {
    if !model_dir.exists() {
        return false;
    }
    // Check config.json exists — other files are referenced from it
    model_dir.join("config.json").exists() && model_dir.join("model.safetensors").exists()
}

/// Get the default model storage directory
pub fn default_model_dir() -> PathBuf {
    dirs_next::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.souffle.app")
        .join("models")
        .join("kyutai")
        .join("stt-1b-en_fr")
}

/// Download model files from HuggingFace using hf-hub.
/// hf-hub handles caching, resumption, and stores files in its own cache.
/// We then symlink/copy the resolved paths to our model dir for easy access.
pub fn download_model(
    model_dir: &Path,
    progress_callback: impl Fn(DownloadProgress),
) -> Result<(), String> {
    std::fs::create_dir_all(model_dir)
        .map_err(|e| format!("Failed to create model dir: {e}"))?;

    let api = hf_hub::api::sync::Api::new()
        .map_err(|e| format!("HuggingFace API init failed: {e}"))?;
    let repo = api.model(KYUTAI_HF_REPO.to_string());

    // First download config.json to discover other file names
    progress_callback(DownloadProgress {
        file: "config.json".into(),
        downloaded_bytes: 0,
        total_bytes: None,
        status: DownloadStatus::Starting,
    });

    let config_path = repo.get("config.json")
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
    let config: serde_json::Value = serde_json::from_str(&config_str)
        .map_err(|e| format!("Invalid config.json: {e}"))?;

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

        let cached_path = repo.get(repo_file)
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
