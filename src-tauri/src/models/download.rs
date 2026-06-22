use std::io::Write;
use std::path::Path;

use tracing::info;

use crate::engine::ModelArtifactDescriptor;

/// Download status reported to the frontend
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct DownloadProgress {
    pub file: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub completed_files: u32,
    pub total_files: u32,
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

/// Check if all required model files exist locally.
/// For engines with a config.json that references extra files (mimi, tokenizer),
/// those referenced files are also checked.
pub fn model_exists(model_dir: &Path, required_files: &[String]) -> bool {
    if !model_dir.exists() {
        return false;
    }
    if !required_files
        .iter()
        .all(|file| model_dir.join(file).exists())
    {
        return false;
    }

    // If config.json exists and references additional files, verify those too
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

/// Download model files from HuggingFace with streaming byte-level progress.
/// Driven by the artifact descriptor's `required_files` list.
/// For Kyutai models that include config.json, additional files
/// referenced in the config (mimi, tokenizer) are also downloaded.
pub fn download_model(
    artifact: &ModelArtifactDescriptor,
    model_dir: &Path,
    progress_callback: impl Fn(DownloadProgress),
) -> Result<(), String> {
    std::fs::create_dir_all(model_dir).map_err(|e| format!("Failed to create model dir: {e}"))?;

    // Build the full file list from required_files + any config.json extras
    let mut files_to_download: Vec<String> = artifact.required_files.clone();

    // If config.json is required, download it first to discover extras
    let has_config = files_to_download.iter().any(|f| f == "config.json");
    if has_config {
        let config_extras = download_and_discover_config(artifact, model_dir, &progress_callback)?;
        for extra in &config_extras {
            if !files_to_download.contains(extra) {
                files_to_download.push(extra.clone());
            }
        }
        // config.json already downloaded
        files_to_download.retain(|f| f != "config.json");
    }

    let total_files = files_to_download.len() as u32 + if has_config { 1 } else { 0 };
    let mut completed_files = if has_config { 1u32 } else { 0u32 };

    progress_callback(DownloadProgress {
        file: files_to_download.first().cloned().unwrap_or_default(),
        downloaded_bytes: 0,
        total_bytes: artifact.download_size_bytes,
        completed_files,
        total_files,
        status: DownloadStatus::Starting,
    });

    let client = reqwest::blocking::Client::builder()
        .user_agent("souffle/0.1")
        .build()
        .map_err(|e| format!("HTTP client init: {e}"))?;

    for repo_file in &files_to_download {
        info!(file = %repo_file, "Downloading");

        let url = hf_download_url(
            &artifact.repository,
            repo_file,
            artifact.revision.as_deref(),
        );
        let dest = model_dir.join(repo_file);

        download_file_with_progress(
            &client,
            &url,
            &dest,
            repo_file,
            completed_files,
            total_files,
            &progress_callback,
        )?;

        completed_files += 1;
        progress_callback(DownloadProgress {
            file: repo_file.clone(),
            downloaded_bytes: 0,
            total_bytes: None,
            completed_files,
            total_files,
            status: DownloadStatus::Complete,
        });
        info!(file = %repo_file, "Download complete");
    }

    info!("All model files downloaded");
    Ok(())
}

/// Download config.json via hf-hub (small file, no progress needed),
/// then discover any extra files it references.
fn download_and_discover_config(
    artifact: &ModelArtifactDescriptor,
    model_dir: &Path,
    progress_callback: &impl Fn(DownloadProgress),
) -> Result<Vec<String>, String> {
    progress_callback(DownloadProgress {
        file: "config.json".into(),
        downloaded_bytes: 0,
        total_bytes: None,
        completed_files: 0,
        total_files: 0,
        status: DownloadStatus::Starting,
    });

    let api = hf_hub::api::sync::Api::new().map_err(|e| format!("HuggingFace API init: {e}"))?;
    let repo = api.model(artifact.repository.clone());

    let config_cached = repo
        .get("config.json")
        .map_err(|e| format!("Failed to download config.json: {e}"))?;

    let dest_config = model_dir.join("config.json");
    std::fs::copy(&config_cached, &dest_config)
        .map_err(|e| format!("Failed to copy config.json: {e}"))?;

    progress_callback(DownloadProgress {
        file: "config.json".into(),
        downloaded_bytes: 0,
        total_bytes: None,
        completed_files: 1,
        total_files: 0,
        status: DownloadStatus::Complete,
    });

    let config_str = std::fs::read_to_string(&dest_config)
        .map_err(|e| format!("Failed to read config.json: {e}"))?;
    let config: serde_json::Value =
        serde_json::from_str(&config_str).map_err(|e| format!("Invalid config.json: {e}"))?;

    let mut extras = Vec::new();
    for field in ["mimi_name", "tokenizer_name"] {
        if let Some(name) = config[field].as_str() {
            extras.push(name.to_string());
        }
    }
    Ok(extras)
}

/// Build the HuggingFace download URL for a file in a repo.
fn hf_download_url(repo_id: &str, filename: &str, revision: Option<&str>) -> String {
    let rev = revision.unwrap_or("main");
    format!("https://huggingface.co/{repo_id}/resolve/{rev}/{filename}")
}

/// Download a single file with streaming byte-level progress.
fn download_file_with_progress(
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    file_label: &str,
    completed_files: u32,
    total_files: u32,
    progress_callback: &impl Fn(DownloadProgress),
) -> Result<(), String> {
    let response = client
        .get(url)
        .send()
        .map_err(|e| format!("HTTP request for {file_label}: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "Download failed for {file_label}: HTTP {}",
            response.status()
        ));
    }

    let total_bytes = response.content_length();

    progress_callback(DownloadProgress {
        file: file_label.to_string(),
        downloaded_bytes: 0,
        total_bytes,
        completed_files,
        total_files,
        status: DownloadStatus::Downloading,
    });

    // Stream to a temp file, then rename
    let temp_dest = dest.with_extension("part");
    let mut file = std::fs::File::create(&temp_dest)
        .map_err(|e| format!("Create temp file for {file_label}: {e}"))?;

    let mut downloaded: u64 = 0;
    let mut last_reported: u64 = 0;
    // Report progress every ~500KB to avoid flooding the channel
    let report_interval: u64 = 512 * 1024;

    let mut reader = std::io::BufReader::new(response);
    let mut buf = [0u8; 64 * 1024];

    loop {
        let n = std::io::Read::read(&mut reader, &mut buf)
            .map_err(|e| format!("Read error for {file_label}: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("Write error for {file_label}: {e}"))?;
        downloaded += n as u64;

        if downloaded - last_reported >= report_interval {
            progress_callback(DownloadProgress {
                file: file_label.to_string(),
                downloaded_bytes: downloaded,
                total_bytes,
                completed_files,
                total_files,
                status: DownloadStatus::Downloading,
            });
            last_reported = downloaded;
        }
    }

    file.flush()
        .map_err(|e| format!("Flush error for {file_label}: {e}"))?;
    drop(file);

    // Atomic rename
    std::fs::rename(&temp_dest, dest).map_err(|e| format!("Rename error for {file_label}: {e}"))?;

    Ok(())
}

/// Build the list of files to download for an artifact.
/// Public for testing — returns (files_needing_download, needs_config_discovery).
pub fn resolve_download_files(artifact: &ModelArtifactDescriptor) -> (Vec<String>, bool) {
    let has_config = artifact.required_files.iter().any(|f| f == "config.json");
    (artifact.required_files.clone(), has_config)
}

/// Delete all model files for a given model directory.
pub fn delete_model_files(model_dir: &Path) -> Result<(), String> {
    if !model_dir.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(model_dir).map_err(|e| format!("Failed to delete model files: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ModelArtifactDescriptor;
    use std::fs;
    use tempfile::TempDir;

    fn kyutai_artifact() -> ModelArtifactDescriptor {
        ModelArtifactDescriptor {
            id: "test-kyutai".into(),
            label: "Test".into(),
            description: "Test artifact".into(),
            provider: "huggingface".into(),
            repository: "test/repo".into(),
            revision: None,
            file_format: "safetensors".into(),
            download_size_bytes: None,
            required_files: vec!["config.json".into(), "model.safetensors".into()],
        }
    }

    fn whisper_artifact() -> ModelArtifactDescriptor {
        ModelArtifactDescriptor {
            id: "test-whisper".into(),
            label: "Test".into(),
            description: "Test artifact".into(),
            provider: "huggingface".into(),
            repository: "test/repo".into(),
            revision: None,
            file_format: "ggml".into(),
            download_size_bytes: Some(1_620_000_000),
            required_files: vec!["ggml-large-v3-turbo.bin".into()],
        }
    }

    #[test]
    fn resolve_download_files_kyutai_needs_config_discovery() {
        let (files, needs_config) = resolve_download_files(&kyutai_artifact());
        assert!(needs_config);
        assert!(files.contains(&"config.json".to_string()));
        assert!(files.contains(&"model.safetensors".to_string()));
    }

    #[test]
    fn resolve_download_files_whisper_no_config_discovery() {
        let (files, needs_config) = resolve_download_files(&whisper_artifact());
        assert!(!needs_config);
        assert_eq!(files.len(), 1);
        assert!(files.contains(&"ggml-large-v3-turbo.bin".to_string()));
    }

    #[test]
    fn model_exists_returns_false_for_missing_dir() {
        assert!(!model_exists(
            std::path::Path::new("/nonexistent/path"),
            &["model.bin".into()]
        ));
    }

    #[test]
    fn model_exists_returns_false_when_required_file_missing() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("other.bin"), "data").unwrap();
        assert!(!model_exists(dir.path(), &["model.bin".into()]));
    }

    #[test]
    fn model_exists_returns_true_when_all_required_files_present() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("model.bin"), "data").unwrap();
        assert!(model_exists(dir.path(), &["model.bin".into()]));
    }

    #[test]
    fn model_exists_returns_true_for_whisper_single_bin() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("ggml-large-v3-turbo.bin"), "data").unwrap();
        assert!(model_exists(
            dir.path(),
            &["ggml-large-v3-turbo.bin".into()]
        ));
    }

    #[test]
    fn model_exists_checks_config_referenced_files() {
        let dir = TempDir::new().unwrap();
        let config = r#"{"mimi_name": "mimi.safetensors", "tokenizer_name": "tok.model"}"#;
        fs::write(dir.path().join("config.json"), config).unwrap();
        fs::write(dir.path().join("model.safetensors"), "data").unwrap();
        assert!(!model_exists(
            dir.path(),
            &["config.json".into(), "model.safetensors".into()]
        ));
    }

    #[test]
    fn model_exists_passes_when_config_referenced_files_present() {
        let dir = TempDir::new().unwrap();
        let config = r#"{"mimi_name": "mimi.safetensors", "tokenizer_name": "tok.model"}"#;
        fs::write(dir.path().join("config.json"), config).unwrap();
        fs::write(dir.path().join("model.safetensors"), "data").unwrap();
        fs::write(dir.path().join("mimi.safetensors"), "data").unwrap();
        fs::write(dir.path().join("tok.model"), "data").unwrap();
        assert!(model_exists(
            dir.path(),
            &["config.json".into(), "model.safetensors".into()]
        ));
    }

    #[test]
    fn model_exists_ignores_config_fields_when_no_config_json() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("ggml-large-v3-turbo.bin"), "data").unwrap();
        assert!(model_exists(
            dir.path(),
            &["ggml-large-v3-turbo.bin".into()]
        ));
    }

    #[test]
    fn hf_download_url_default_revision() {
        let url = hf_download_url("ggerganov/whisper.cpp", "ggml-large-v3-turbo.bin", None);
        assert_eq!(
            url,
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin"
        );
    }

    #[test]
    fn hf_download_url_custom_revision() {
        let url = hf_download_url(
            "kyutai/stt-1b-en_fr-candle",
            "model.safetensors",
            Some("v1.0"),
        );
        assert_eq!(
            url,
            "https://huggingface.co/kyutai/stt-1b-en_fr-candle/resolve/v1.0/model.safetensors"
        );
    }

    #[test]
    fn delete_model_files_nonexistent_dir_ok() {
        assert!(delete_model_files(std::path::Path::new("/nonexistent/delete/test")).is_ok());
    }

    #[test]
    fn delete_model_files_removes_directory() {
        let dir = TempDir::new().unwrap();
        let model_dir = dir.path().join("model");
        fs::create_dir_all(&model_dir).unwrap();
        fs::write(model_dir.join("model.bin"), "data").unwrap();
        assert!(delete_model_files(&model_dir).is_ok());
        assert!(!model_dir.exists());
    }
}
