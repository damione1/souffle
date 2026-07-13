//! pyannote/segmentation-3.0 ONNX inference: sliding-window powerset
//! decoding, cross-window aggregation, and onset/offset binarization into
//! raw (unclustered) speech segments.
//!
//! Model constants below (`WINDOW_SIZE`, `RECEPTIVE_FIELD_SHIFT`,
//! `RECEPTIVE_FIELD_SIZE`, `NUM_LOCAL_SPEAKERS`, `POWERSET_MAX_CLASSES`) are
//! read from the ONNX file's own embedded metadata (verified with
//! `onnx.load(...).metadata_props` against the artifact declared in
//! `crate::diarize::models::segmentation_artifact`), not guessed: sample_rate
//! =16000, window_size=160000, receptive_field_size=991,
//! receptive_field_shift=270, num_speakers=3, powerset_max_classes=2,
//! num_classes=7. `WINDOW_SHIFT` is 10% of `WINDOW_SIZE`, per
//! `offline-speaker-segmentation-pyannote-model.cc` in sherpa-onnx.

use std::path::Path;

use ndarray::{Array1, Axis};
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Value;

use super::DiarizeError;
use super::aggregate::{BinarizeParams, aggregate_windows, binarize_curve};
use super::powerset::{build_powerset_mapping, decode_frame};

pub const SAMPLE_RATE: u32 = 16_000;
const WINDOW_SIZE: usize = 160_000;
const WINDOW_SHIFT: usize = 16_000;
const RECEPTIVE_FIELD_SHIFT: usize = 270;
const RECEPTIVE_FIELD_SIZE: usize = 991;
const NUM_LOCAL_SPEAKERS: usize = 3;
const POWERSET_MAX_CLASSES: usize = 2;

pub struct SegmentationModel {
    session: Session,
}

impl SegmentationModel {
    pub fn load(path: &Path) -> Result<Self, DiarizeError> {
        crate::ort_runtime::ensure_ort_initialized();

        let session = Session::builder()
            .map_err(|e| DiarizeError::ModelLoad(e.to_string()))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| DiarizeError::ModelLoad(e.to_string()))?
            .with_intra_threads(4)
            .map_err(|e| DiarizeError::ModelLoad(e.to_string()))?
            .commit_from_file(path)
            .map_err(|e| DiarizeError::ModelLoad(format!("{}: {e}", path.display())))?;

        Ok(Self { session })
    }

    /// Run one window (zero-padded to exactly `WINDOW_SIZE` samples) through
    /// the model, returning per-frame powerset class logits.
    fn run_window(&mut self, window: &[f32]) -> Result<Vec<Vec<f32>>, DiarizeError> {
        debug_assert_eq!(window.len(), WINDOW_SIZE);

        let array = Array1::from_vec(window.to_vec())
            .insert_axis(Axis(0))
            .insert_axis(Axis(1));
        let input = Value::from_array(array).map_err(|e| DiarizeError::Inference(e.to_string()))?;

        let outputs = self
            .session
            .run(ort::inputs!["x" => input])
            .map_err(|e| DiarizeError::Inference(e.to_string()))?;
        let output = outputs
            .get("y")
            .ok_or_else(|| DiarizeError::Inference("segmentation model has no 'y' output".into()))?;
        let (shape, data) = output
            .try_extract_tensor::<f32>()
            .map_err(|e| DiarizeError::Inference(e.to_string()))?;

        let num_frames = shape[1] as usize;
        let num_classes = shape[2] as usize;
        Ok(data.chunks(num_classes).take(num_frames).map(<[f32]>::to_vec).collect())
    }
}

/// Run the sliding-window segmentation pipeline end to end: windowing,
/// powerset decode, cross-window aggregation, and hysteresis binarization.
/// Returns raw `(start_s, end_s)` speech segments merged from all local
/// speaker slots. Slot identity is discarded here since it is only a
/// within-window bookkeeping index, not a stable global speaker id (that is
/// resolved later by embedding + clustering each segment).
pub fn detect_speech_segments(
    model: &mut SegmentationModel,
    samples: &[f32],
    binarize_params: &BinarizeParams,
) -> Result<Vec<(f64, f64)>, DiarizeError> {
    if samples.is_empty() {
        return Ok(Vec::new());
    }

    let mapping = build_powerset_mapping(NUM_LOCAL_SPEAKERS, POWERSET_MAX_CLASSES);
    let mut window_scores: Vec<Vec<Vec<f32>>> = Vec::new();

    let mut start = 0usize;
    loop {
        let end = (start + WINDOW_SIZE).min(samples.len());
        let mut window = samples[start..end].to_vec();
        window.resize(WINDOW_SIZE, 0.0);

        let logits = model.run_window(&window)?;
        let frame_scores: Vec<Vec<f32>> = logits
            .iter()
            .map(|frame_logits| decode_frame(frame_logits, &mapping, NUM_LOCAL_SPEAKERS))
            .collect();
        window_scores.push(frame_scores);

        if end >= samples.len() {
            break;
        }
        start += WINDOW_SHIFT;
    }

    let curves = aggregate_windows(
        &window_scores,
        WINDOW_SHIFT as u64,
        RECEPTIVE_FIELD_SHIFT as u64,
        NUM_LOCAL_SPEAKERS,
    );

    let frame_duration_s = RECEPTIVE_FIELD_SHIFT as f64 / SAMPLE_RATE as f64;
    // Segment boundaries land on the center of each frame's receptive field,
    // matching sherpa-onnx's `scale_offset = 0.5 * receptive_field_size /
    // sample_rate` in `ComputeResult`.
    let frame_offset_s = 0.5 * RECEPTIVE_FIELD_SIZE as f64 / SAMPLE_RATE as f64;

    let mut segments = Vec::new();
    for curve in &curves {
        segments.extend(binarize_curve(curve, frame_duration_s, frame_offset_s, binarize_params));
    }
    segments.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(segments)
}
