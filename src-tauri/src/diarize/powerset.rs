//! Powerset decoding for pyannote's segmentation-3.0 output.
//!
//! The segmentation model does not output one activity score per speaker.
//! Instead each frame is classified into one "powerset" class covering every
//! combination of up to `powerset_max_classes` simultaneously active local
//! speakers out of `num_speakers` slots (class 0 = silence). This mirrors
//! `Powerset` in pyannote-audio and `InitPowersetMapping` in sherpa-onnx:
//! https://github.com/pyannote/pyannote-audio/blob/develop/pyannote/audio/utils/powerset.py

/// Build the class -> active-speaker-indices table for a powerset encoding.
/// Class 0 is always silence (no speakers active). Classes
/// `1..=num_speakers` are single speakers, enumerated in slot order. The
/// remaining classes (only emitted when `powerset_max_classes == 2`, which is
/// all pyannote/segmentation-3.0 needs) are every unordered pair of speakers,
/// enumerated with the first slot index increasing slowest.
///
/// Returns one entry per class; entry `i` lists the local speaker slots
/// active in class `i`.
pub fn build_powerset_mapping(num_speakers: usize, powerset_max_classes: usize) -> Vec<Vec<usize>> {
    assert!(
        (1..=2).contains(&powerset_max_classes),
        "only powerset_max_classes 1 or 2 is supported, got {powerset_max_classes}"
    );

    let mut mapping = vec![Vec::new()]; // class 0: silence

    for speaker in 0..num_speakers {
        mapping.push(vec![speaker]);
    }

    if powerset_max_classes == 2 {
        for a in 0..num_speakers {
            for b in (a + 1)..num_speakers {
                mapping.push(vec![a, b]);
            }
        }
    }

    mapping
}

/// Decode one frame's powerset class logits into a per-local-speaker soft
/// activity probability: softmax over classes, then for each speaker sum the
/// probability mass of every class that includes it. This is the standard
/// pyannote-audio "soft" multilabel conversion (as opposed to a hard argmax
/// decode), which is what the onset/offset hysteresis binarizer downstream
/// expects to threshold against.
pub fn decode_frame(logits: &[f32], mapping: &[Vec<usize>], num_speakers: usize) -> Vec<f32> {
    debug_assert_eq!(logits.len(), mapping.len());

    let max_logit = logits.iter().copied().fold(f32::MIN, f32::max);
    let exp: Vec<f32> = logits.iter().map(|&l| (l - max_logit).exp()).collect();
    let sum: f32 = exp.iter().sum();
    let probs: Vec<f32> = if sum > 0.0 {
        exp.iter().map(|&e| e / sum).collect()
    } else {
        vec![0.0; exp.len()]
    };

    let mut speaker_probs = vec![0.0f32; num_speakers];
    for (class_probs, active) in probs.iter().zip(mapping.iter()) {
        for &speaker in active {
            speaker_probs[speaker] += class_probs;
        }
    }
    speaker_probs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_for_3_speakers_max_2_has_7_classes() {
        let mapping = build_powerset_mapping(3, 2);
        assert_eq!(mapping.len(), 7);
        assert_eq!(mapping[0], Vec::<usize>::new());
        assert_eq!(mapping[1], vec![0]);
        assert_eq!(mapping[2], vec![1]);
        assert_eq!(mapping[3], vec![2]);
        assert_eq!(mapping[4], vec![0, 1]);
        assert_eq!(mapping[5], vec![0, 2]);
        assert_eq!(mapping[6], vec![1, 2]);
    }

    #[test]
    fn mapping_for_powerset_max_1_has_only_singles() {
        let mapping = build_powerset_mapping(3, 1);
        assert_eq!(mapping.len(), 4);
        assert_eq!(mapping[1], vec![0]);
        assert_eq!(mapping[3], vec![2]);
    }

    #[test]
    #[should_panic(expected = "powerset_max_classes")]
    fn mapping_rejects_unsupported_max_classes() {
        build_powerset_mapping(3, 3);
    }

    #[test]
    fn decode_frame_silence_class_yields_near_zero_activity() {
        let mapping = build_powerset_mapping(3, 2);
        let mut logits = vec![-10.0f32; 7];
        logits[0] = 10.0; // silence class dominates
        let probs = decode_frame(&logits, &mapping, 3);
        assert!(probs.iter().all(|&p| p < 0.01));
    }

    #[test]
    fn decode_frame_pair_class_activates_both_speakers() {
        let mapping = build_powerset_mapping(3, 2);
        let mut logits = vec![-10.0f32; 7];
        logits[4] = 10.0; // class {0,1}
        let probs = decode_frame(&logits, &mapping, 3);
        assert!(probs[0] > 0.98);
        assert!(probs[1] > 0.98);
        assert!(probs[2] < 0.01);
    }

    #[test]
    fn decode_frame_probabilities_sum_consistently_with_class_mass() {
        // Uniform logits: every class gets 1/7 mass. Speaker 0 appears in
        // classes {1, 4, 5} -> should get 3/7 activity.
        let mapping = build_powerset_mapping(3, 2);
        let logits = vec![0.0f32; 7];
        let probs = decode_frame(&logits, &mapping, 3);
        assert!((probs[0] - 3.0 / 7.0).abs() < 1e-5);
    }
}
