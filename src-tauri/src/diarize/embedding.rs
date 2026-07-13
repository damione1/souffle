//! WeSpeaker ResNet34-LM ONNX inference: fbank features in, a 256-dim
//! speaker embedding out. Input tensor name is `feats` (B, T, 80), output is
//! `embs` (B, 256), verified against the artifact declared in
//! `crate::diarize::models::embedding_artifact`.

use std::path::Path;

use ndarray::Array3;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Value;

use super::DiarizeError;
use super::fbank::FbankExtractor;

pub const EMBEDDING_DIM: usize = 256;

pub struct EmbeddingModel {
    session: Session,
    fbank: FbankExtractor,
}

impl EmbeddingModel {
    pub fn load(path: &Path, sample_rate: u32) -> Result<Self, DiarizeError> {
        crate::ort_runtime::ensure_ort_initialized();

        let session = Session::builder()
            .map_err(|e| DiarizeError::ModelLoad(e.to_string()))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| DiarizeError::ModelLoad(e.to_string()))?
            .with_intra_threads(4)
            .map_err(|e| DiarizeError::ModelLoad(e.to_string()))?
            .commit_from_file(path)
            .map_err(|e| DiarizeError::ModelLoad(format!("{}: {e}", path.display())))?;

        Ok(Self {
            session,
            fbank: FbankExtractor::new(sample_rate),
        })
    }

    /// Compute a 256-dim speaker embedding for one audio segment. Returns
    /// `None` when the segment is too short to yield any fbank frames, or
    /// when the model outputs NaN (can happen on degenerate near-silent
    /// input; the caller should just drop the segment rather than let a NaN
    /// poison clustering).
    pub fn embed(&mut self, samples: &[f32]) -> Result<Option<Vec<f32>>, DiarizeError> {
        let features = self.fbank.compute(samples);
        if features.is_empty() {
            return Ok(None);
        }

        let num_frames = features.len();
        let dim = features[0].len();
        let flat: Vec<f32> = features.into_iter().flatten().collect();
        let array = Array3::from_shape_vec((1, num_frames, dim), flat)
            .map_err(|e| DiarizeError::Inference(e.to_string()))?;
        let input = Value::from_array(array).map_err(|e| DiarizeError::Inference(e.to_string()))?;

        let outputs = self
            .session
            .run(ort::inputs!["feats" => input])
            .map_err(|e| DiarizeError::Inference(e.to_string()))?;
        let output = outputs
            .get("embs")
            .ok_or_else(|| DiarizeError::Inference("embedding model has no 'embs' output".into()))?;
        let (_, data) = output
            .try_extract_tensor::<f32>()
            .map_err(|e| DiarizeError::Inference(e.to_string()))?;

        if data.iter().any(|v| v.is_nan()) {
            return Ok(None);
        }

        Ok(Some(data.to_vec()))
    }
}
