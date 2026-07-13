//! Model artifact declarations and on-disk layout for the two ONNX models
//! diarization needs. Downloaded on demand the same way transcription models
//! are (see `crate::models`), but diarization is not a `TranscriptionProfile`
//! so it keeps its own small catalog instead of joining that one.

use std::path::PathBuf;

use crate::engine::ModelArtifactDescriptor;
use crate::models::download::{self, DownloadProgress};

const SEGMENTATION_DIR_NAME: &str = "diarize-segmentation-pyannote-3-0";
const EMBEDDING_DIR_NAME: &str = "diarize-embedding-wespeaker-resnet34-lm";

const SEGMENTATION_FILENAME: &str = "model.onnx";
const EMBEDDING_FILENAME: &str = "wespeaker_en_voxceleb_resnet34_LM.onnx";

/// pyannote/segmentation-3.0 (MIT license,
/// https://huggingface.co/pyannote/segmentation-3.0/blob/main/LICENSE),
/// converted to ONNX by the k2-fsa/sherpa-onnx maintainers and re-hosted
/// (same MIT license) at
/// https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0 .
/// Verified empirically: `model.onnx` is 5,992,913 bytes and its embedded
/// metadata (`sample_rate=16000, window_size=160000, num_speakers=3,
/// powerset_max_classes=2, num_classes=7, receptive_field_size=991,
/// receptive_field_shift=270`) matches what `crate::diarize::segmentation`
/// assumes.
pub fn segmentation_artifact() -> ModelArtifactDescriptor {
    ModelArtifactDescriptor {
        id: "diarize-segmentation-pyannote-3-0-onnx".to_string(),
        label: "pyannote segmentation-3.0 (ONNX)".to_string(),
        description: "Local speaker-change / voice-activity segmentation model, powerset-encoded, 3 local speaker slots.".to_string(),
        provider: "huggingface".to_string(),
        repository: "csukuangfj/sherpa-onnx-pyannote-segmentation-3-0".to_string(),
        revision: None,
        file_format: "onnx".to_string(),
        download_size_bytes: Some(5_992_913),
        required_files: vec![SEGMENTATION_FILENAME.to_string()],
    }
}

/// pyannote/wespeaker-voxceleb-resnet34-LM (CC-BY-4.0,
/// https://huggingface.co/pyannote/wespeaker-voxceleb-resnet34-LM), a
/// pyannote.audio wrapper around WeSpeaker's `wespeaker-voxceleb-resnet34-LM`
/// pretrained speaker embedding model (https://github.com/wenet-e2e/wespeaker).
/// ONNX export re-hosted (same license chain) at
/// https://huggingface.co/csukuangfj/speaker-embedding-models . Verified
/// empirically: `wespeaker_en_voxceleb_resnet34_LM.onnx` is 26,530,550 bytes
/// with input `feats` (B, T, 80) and output `embs` (B, 256).
pub fn embedding_artifact() -> ModelArtifactDescriptor {
    ModelArtifactDescriptor {
        id: "diarize-embedding-wespeaker-resnet34-lm-onnx".to_string(),
        label: "WeSpeaker ResNet34-LM (ONNX)".to_string(),
        description: "256-dim speaker embedding model trained on VoxCeleb.".to_string(),
        provider: "huggingface".to_string(),
        repository: "csukuangfj/speaker-embedding-models".to_string(),
        revision: None,
        file_format: "onnx".to_string(),
        download_size_bytes: Some(26_530_550),
        required_files: vec![EMBEDDING_FILENAME.to_string()],
    }
}

fn diarize_models_root() -> PathBuf {
    crate::constants::app_data_dir().join("models").join("diarize")
}

pub fn segmentation_model_dir() -> PathBuf {
    diarize_models_root().join(SEGMENTATION_DIR_NAME)
}

pub fn embedding_model_dir() -> PathBuf {
    diarize_models_root().join(EMBEDDING_DIR_NAME)
}

pub fn segmentation_model_path() -> PathBuf {
    segmentation_model_dir().join(SEGMENTATION_FILENAME)
}

pub fn embedding_model_path() -> PathBuf {
    embedding_model_dir().join(EMBEDDING_FILENAME)
}

/// Whether both diarization models are already downloaded.
pub fn models_downloaded() -> bool {
    download::model_exists(&segmentation_model_dir(), &segmentation_artifact().required_files)
        && download::model_exists(&embedding_model_dir(), &embedding_artifact().required_files)
}

/// Download both diarization models, reporting progress for each file
/// through the same callback used by `crate::models::download_model`.
pub fn download_models(progress_callback: &dyn Fn(DownloadProgress)) -> Result<(), String> {
    download::download_model(&segmentation_artifact(), &segmentation_model_dir(), progress_callback)?;
    download::download_model(&embedding_artifact(), &embedding_model_dir(), progress_callback)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segmentation_artifact_declares_one_file() {
        let artifact = segmentation_artifact();
        assert_eq!(artifact.required_files, vec![SEGMENTATION_FILENAME.to_string()]);
        assert_eq!(artifact.download_size_bytes, Some(5_992_913));
    }

    #[test]
    fn embedding_artifact_declares_one_file() {
        let artifact = embedding_artifact();
        assert_eq!(artifact.required_files, vec![EMBEDDING_FILENAME.to_string()]);
        assert_eq!(artifact.download_size_bytes, Some(26_530_550));
    }

    #[test]
    fn model_dirs_are_distinct_and_under_diarize_root() {
        let seg = segmentation_model_dir();
        let emb = embedding_model_dir();
        assert_ne!(seg, emb);
        assert!(seg.ends_with(SEGMENTATION_DIR_NAME));
        assert!(emb.ends_with(EMBEDDING_DIR_NAME));
    }

    #[test]
    fn model_paths_join_the_expected_filename() {
        assert_eq!(segmentation_model_path().file_name().unwrap(), SEGMENTATION_FILENAME);
        assert_eq!(embedding_model_path().file_name().unwrap(), EMBEDDING_FILENAME);
    }
}
