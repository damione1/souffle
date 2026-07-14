//! Offline speaker diarization: given a full recording, decide who spoke
//! when. Runs after the fact on the whole file (not during live capture), so
//! latency is not a concern, only throughput (target: roughly 1 minute of
//! CPU time per hour of audio).
//!
//! Three-stage pipeline, industry-standard for pyannote-family diarization:
//! 1. `segmentation`: pyannote/segmentation-3.0 (ONNX) finds who-is-speaking
//!    intervals via sliding-window powerset decoding.
//! 2. `embedding`: WeSpeaker ResNet34-LM (ONNX) turns each interval into a
//!    256-dim speaker embedding, via the `fbank` frontend.
//! 3. `clustering`: cosine-similarity greedy agglomerative clustering groups
//!    same-speaker segments together.
//!
//! This module implements the core algorithm only. Wiring it into the
//! meeting pipeline (tapping mic-only meeting audio, matching clusters
//! against persistent speakers, and assigning transcript segments) lives in
//! `persist`, `assign`, `crate::audio::diarize_tap`, and
//! `crate::pipeline::diarize_task`. See `crate::cli`'s `--diarize-file` flag
//! for a standalone way to exercise the core algorithm directly.

pub mod aggregate;
pub mod assign;
pub mod clustering;
pub mod embedding;
pub mod fbank;
pub mod models;
pub mod persist;
pub mod powerset;
pub mod segmentation;

use std::path::PathBuf;

use aggregate::BinarizeParams;
use embedding::EmbeddingModel;
use segmentation::SegmentationModel;

#[derive(Debug, thiserror::Error)]
pub enum DiarizeError {
    #[error("Failed to load model: {0}")]
    ModelLoad(String),
    #[error("Inference failed: {0}")]
    Inference(String),
    #[error("Invalid audio: {0}")]
    InvalidAudio(String),
}

/// Model paths and tunable thresholds for one `diarize()` run.
#[derive(Debug, Clone)]
pub struct DiarizeConfig {
    pub segmentation_model_path: PathBuf,
    pub embedding_model_path: PathBuf,
    /// Activity score above which a local speaker slot is considered to have
    /// started speaking.
    pub onset: f32,
    /// Activity score below which a local speaker slot is considered to have
    /// stopped speaking. `offset <= onset` gives hysteresis; equal values
    /// (the default) behave as a plain single threshold.
    pub offset: f32,
    /// Segments shorter than this are dropped after gap merging.
    pub min_duration_on_s: f64,
    /// Gaps shorter than this between same-slot segments are merged.
    pub min_duration_off_s: f64,
    /// Cosine similarity above which two speaker embeddings are merged into
    /// the same cluster.
    pub cluster_threshold: f32,
    pub min_speakers: Option<usize>,
    pub max_speakers: Option<usize>,
}

impl DiarizeConfig {
    pub fn new(segmentation_model_path: PathBuf, embedding_model_path: PathBuf) -> Self {
        Self {
            segmentation_model_path,
            embedding_model_path,
            onset: 0.5,
            offset: 0.5,
            min_duration_on_s: 0.1,
            min_duration_off_s: 0.5,
            cluster_threshold: 0.7,
            min_speakers: None,
            max_speakers: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiarizedSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker: usize,
}

/// A detected speaker's embedding centroid (mean of its member segments'
/// L2-normalized embeddings, re-normalized). Kept in the result (rather than
/// discarded once clustering is done) so a follow-up feature can persist
/// speaker identity across meetings by comparing centroids, without
/// re-running inference on old recordings.
#[derive(Debug, Clone)]
pub struct SpeakerCentroid {
    pub speaker: usize,
    pub embedding: Vec<f32>,
}

/// `diarize()`'s output. Deviates from a bare `Vec<DiarizedSegment>` on
/// purpose: dropping the per-speaker centroids here would make them
/// unrecoverable without re-running the embedding model over the whole
/// recording again.
#[derive(Debug, Clone)]
pub struct DiarizationResult {
    pub segments: Vec<DiarizedSegment>,
    pub speakers: Vec<SpeakerCentroid>,
}

/// Segments shorter than this are not worth embedding: too little audio for
/// a stable speaker embedding, and too short to matter in a transcript.
/// 0.25s at 16kHz, matching sherpa-onnx's own short-segment skip.
const MIN_SEGMENT_SAMPLES_FOR_EMBEDDING: usize = 4_000;

/// Run offline speaker diarization on a full, already-decoded recording.
///
/// `samples` must be mono f32 PCM at `sample_rate`, which must equal
/// `segmentation::SAMPLE_RATE` (16000 Hz); both ONNX models are fixed to
/// that rate. Resample before calling, e.g. with `crate::audio::Resampler`;
/// resampling is deliberately the caller's responsibility, not this
/// function's, so this module stays a pure post-hoc analysis step over
/// audio the caller already owns.
pub fn diarize(samples: &[f32], sample_rate: u32, cfg: &DiarizeConfig) -> Result<DiarizationResult, DiarizeError> {
    if sample_rate != segmentation::SAMPLE_RATE {
        return Err(DiarizeError::InvalidAudio(format!(
            "diarize() requires {}Hz mono audio, got {sample_rate}Hz",
            segmentation::SAMPLE_RATE
        )));
    }
    if samples.is_empty() {
        return Ok(DiarizationResult {
            segments: Vec::new(),
            speakers: Vec::new(),
        });
    }

    let raw_segments = {
        let mut seg_model = SegmentationModel::load(&cfg.segmentation_model_path)?;
        let binarize_params = BinarizeParams {
            onset: cfg.onset,
            offset: cfg.offset,
            min_duration_on_s: cfg.min_duration_on_s,
            min_duration_off_s: cfg.min_duration_off_s,
        };
        segmentation::detect_speech_segments(&mut seg_model, samples, &binarize_params)?
    };

    if raw_segments.is_empty() {
        return Ok(DiarizationResult {
            segments: Vec::new(),
            speakers: Vec::new(),
        });
    }

    let (kept_segments, embeddings) = {
        let mut emb_model = EmbeddingModel::load(&cfg.embedding_model_path, sample_rate)?;
        let mut kept_segments = Vec::with_capacity(raw_segments.len());
        let mut embeddings = Vec::with_capacity(raw_segments.len());

        for (start_s, end_s) in raw_segments {
            let start_sample = (start_s * sample_rate as f64).round().max(0.0) as usize;
            let end_sample = ((end_s * sample_rate as f64).round() as usize).min(samples.len());
            if end_sample <= start_sample || end_sample - start_sample < MIN_SEGMENT_SAMPLES_FOR_EMBEDDING {
                continue;
            }

            let Some(embedding) = emb_model.embed(&samples[start_sample..end_sample])? else {
                continue;
            };

            kept_segments.push((start_s, end_s));
            embeddings.push(embedding);
        }
        (kept_segments, embeddings)
    };

    if embeddings.is_empty() {
        return Ok(DiarizationResult {
            segments: Vec::new(),
            speakers: Vec::new(),
        });
    }

    let cluster_result = clustering::cluster(&embeddings, cfg.cluster_threshold, cfg.min_speakers, cfg.max_speakers);

    let mut segments: Vec<DiarizedSegment> = kept_segments
        .iter()
        .zip(cluster_result.labels.iter())
        .map(|(&(start_s, end_s), &speaker)| DiarizedSegment {
            start_ms: (start_s * 1000.0).round() as u64,
            end_ms: (end_s * 1000.0).round() as u64,
            speaker,
        })
        .collect();
    segments.sort_by_key(|s| s.start_ms);

    let max_gap_ms = (cfg.min_duration_off_s * 1000.0).round() as u64;
    let segments = merge_adjacent_same_speaker(segments, max_gap_ms);

    let speakers = cluster_result
        .centroids
        .into_iter()
        .enumerate()
        .map(|(speaker, embedding)| SpeakerCentroid { speaker, embedding })
        .collect();

    Ok(DiarizationResult { segments, speakers })
}

/// Merge same-speaker segments whose gap is at most `max_gap_ms`. Final
/// cleanup pass: clustering can leave back-to-back segments from the same
/// real speaker that the segmentation stage originally split across two
/// different local speaker slots. `segments` must already be sorted by
/// `start_ms`.
fn merge_adjacent_same_speaker(segments: Vec<DiarizedSegment>, max_gap_ms: u64) -> Vec<DiarizedSegment> {
    let mut merged: Vec<DiarizedSegment> = Vec::with_capacity(segments.len());
    for seg in segments {
        if let Some(last) = merged.last_mut()
            && last.speaker == seg.speaker
            && seg.start_ms.saturating_sub(last.end_ms) <= max_gap_ms
        {
            last.end_ms = last.end_ms.max(seg.end_ms);
            continue;
        }
        merged.push(seg);
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_adjacent_same_speaker_joins_close_segments() {
        let segments = vec![
            DiarizedSegment { start_ms: 0, end_ms: 1_000, speaker: 0 },
            DiarizedSegment { start_ms: 1_200, end_ms: 2_000, speaker: 0 },
            DiarizedSegment { start_ms: 2_000, end_ms: 3_000, speaker: 1 },
        ];
        let merged = merge_adjacent_same_speaker(segments, 500);
        assert_eq!(
            merged,
            vec![
                DiarizedSegment { start_ms: 0, end_ms: 2_000, speaker: 0 },
                DiarizedSegment { start_ms: 2_000, end_ms: 3_000, speaker: 1 },
            ]
        );
    }

    #[test]
    fn merge_adjacent_same_speaker_keeps_far_apart_segments_separate() {
        let segments = vec![
            DiarizedSegment { start_ms: 0, end_ms: 1_000, speaker: 0 },
            DiarizedSegment { start_ms: 5_000, end_ms: 6_000, speaker: 0 },
        ];
        let merged = merge_adjacent_same_speaker(segments, 500);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merge_adjacent_same_speaker_empty_input() {
        assert!(merge_adjacent_same_speaker(Vec::new(), 500).is_empty());
    }

    #[test]
    fn diarize_rejects_wrong_sample_rate() {
        let cfg = DiarizeConfig::new(PathBuf::from("seg.onnx"), PathBuf::from("emb.onnx"));
        let result = diarize(&[0.0f32; 1_000], 44_100, &cfg);
        assert!(matches!(result, Err(DiarizeError::InvalidAudio(_))));
    }

    #[test]
    fn diarize_empty_audio_returns_empty_result_without_touching_models() {
        // Model paths are bogus on purpose: the empty-audio short circuit
        // must return before ever trying to load them.
        let cfg = DiarizeConfig::new(PathBuf::from("/nonexistent/seg.onnx"), PathBuf::from("/nonexistent/emb.onnx"));
        let result = diarize(&[], 16_000, &cfg).unwrap();
        assert!(result.segments.is_empty());
        assert!(result.speakers.is_empty());
    }

    /// End-to-end pipeline against the real downloaded models. Requires both
    /// diarization models in the app models dir and a 16kHz mono test WAV at
    /// `/tmp/diarize_test.wav`. Run with:
    ///   cargo test diarize_real_pipeline -- --ignored --nocapture
    #[test]
    #[ignore = "requires downloaded diarization models (~32MB) and a test WAV"]
    fn diarize_real_pipeline() {
        assert!(
            models::models_downloaded(),
            "diarization models missing, download them first"
        );
        let cfg = DiarizeConfig::new(models::segmentation_model_path(), models::embedding_model_path());

        let mut reader = hound::WavReader::open("/tmp/diarize_test.wav")
            .expect("test WAV missing, synthesize a two-voice one with `say` at 16kHz mono");
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 16_000);
        // 16-bit PCM (what `afconvert`/`say` produce) and 32-bit float are
        // both plausible for a hand-built test WAV; read whichever this one is.
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Int => reader
                .samples::<i16>()
                .filter_map(|s| s.ok())
                .map(|s| s as f32 / i16::MAX as f32)
                .collect(),
            hound::SampleFormat::Float => reader.samples::<f32>().filter_map(|s| s.ok()).collect(),
        };

        let result = diarize(&samples, 16_000, &cfg).expect("diarize");
        eprintln!(
            "Detected {} speaker(s), {} segments",
            result.speakers.len(),
            result.segments.len()
        );
        for seg in &result.segments {
            eprintln!("  [{:>7}ms .. {:>7}ms] speaker {}", seg.start_ms, seg.end_ms, seg.speaker);
        }
        assert!(!result.segments.is_empty(), "expected at least one segment");
    }
}
