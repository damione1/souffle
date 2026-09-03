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

    relocate_model_directory(&legacy_dir, &target_dir)
}

/// Move a legacy model directory to `target`.
///
/// Kyutai's candle layout is `…/stt-1b-en_fr/candle`, i.e. a *child* of the
/// legacy dir `…/stt-1b-en_fr`. `rename(legacy, legacy/candle)` is EINVAL, and
/// a naive recursive copy then copies the target into itself
/// (`candle/candle/…`). Never rename or recurse a directory into its own
/// descendant — lift files instead.
fn relocate_model_directory(source: &Path, target: &Path) -> Result<(), String> {
    if source == target {
        return Ok(());
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create model parent directory: {e}"))?;
    }

    if target.starts_with(source) {
        return copy_directory_contents(source, target);
    }

    match std::fs::rename(source, target) {
        Ok(()) => Ok(()),
        Err(_) => copy_directory_contents(source, target),
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
    if source == target {
        return Ok(());
    }

    std::fs::create_dir_all(target).map_err(|e| format!("Failed to create target dir: {e}"))?;

    for entry in std::fs::read_dir(source).map_err(|e| format!("Failed to read source dir: {e}"))? {
        let entry = entry.map_err(|e| format!("Failed to read source entry: {e}"))?;
        let entry_type = entry
            .file_type()
            .map_err(|e| format!("Failed to inspect source entry type: {e}"))?;
        let source_path = entry.path();

        // Target may be a child of source (legacy Kyutai → `…/candle`).
        // Creating `target` first would otherwise make `read_dir` see it and
        // recurse `candle/candle/…` without bound.
        if source_path == target || source_path.starts_with(target) {
            continue;
        }

        let target_path = target.join(entry.file_name());

        if entry_type.is_dir() {
            copy_directory_contents(&source_path, &target_path)?;
            continue;
        }

        match std::fs::rename(&source_path, &target_path) {
            Ok(()) => {}
            Err(_) => {
                std::fs::copy(&source_path, &target_path).map_err(|e| {
                    format!("Failed to copy model file '{}': {e}", source_path.display())
                })?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn count_files(dir: &Path) -> usize {
        if !dir.is_dir() {
            return usize::from(dir.is_file());
        }
        let mut n = 0;
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                n += count_files(&path);
            } else {
                n += 1;
            }
        }
        n
    }

    fn max_candle_depth(dir: &Path) -> usize {
        let mut depth = 0;
        let mut cursor = dir.to_path_buf();
        while cursor.join("candle").is_dir() {
            depth += 1;
            cursor = cursor.join("candle");
        }
        depth
    }

    #[test]
    fn relocate_into_own_child_does_not_nest_candle() {
        let tmp = TempDir::new().unwrap();
        let legacy = tmp.path().join("kyutai").join("stt-1b-en_fr");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("config.json"), "{\"dim\":1}").unwrap();
        fs::write(legacy.join("model.safetensors"), b"weights").unwrap();
        // Pre-existing child — the layout that made the old copy explode.
        fs::create_dir_all(legacy.join("candle")).unwrap();
        fs::write(legacy.join("candle").join("already.txt"), "keep").unwrap();

        let target = legacy.join("candle");
        relocate_model_directory(&legacy, &target).unwrap();

        assert!(target.join("config.json").is_file());
        assert!(target.join("model.safetensors").is_file());
        assert!(target.join("already.txt").is_file());
        assert!(!target.join("candle").exists(), "nested candle/candle");
        assert_eq!(max_candle_depth(&legacy), 1);
        assert!(
            count_files(tmp.path()) <= 4,
            "unbounded copy: {} files",
            count_files(tmp.path())
        );
    }

    #[test]
    fn relocate_creates_child_target_without_recursing() {
        let tmp = TempDir::new().unwrap();
        let legacy = tmp.path().join("stt-1b-en_fr");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("config.json"), "{}").unwrap();
        fs::write(legacy.join("model.safetensors"), b"w").unwrap();

        let target = legacy.join("candle");
        relocate_model_directory(&legacy, &target).unwrap();

        assert!(target.join("config.json").is_file());
        assert!(target.join("model.safetensors").is_file());
        assert!(!target.join("candle").exists());
        assert_eq!(count_files(&target), 2);
    }

    #[test]
    fn relocate_same_path_is_noop() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("model");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.json"), "{}").unwrap();
        relocate_model_directory(&dir, &dir).unwrap();
        assert!(dir.join("config.json").is_file());
        assert_eq!(count_files(&dir), 1);
    }
}
