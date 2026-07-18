//! Matches a `diarize()` run's per-meeting speaker clusters against
//! persistent, cross-meeting speaker identities stored in the `speakers`
//! table, so the same voice resolves to the same speaker row across
//! meetings. Pure decision logic only: no I/O, no database access. The
//! caller (`pipeline::diarize_task`) is responsible for turning the returned
//! `MatchDecision`s into `create_speaker`/`append_speaker_embedding` calls.
//!
//! Matching is against a bag of recent embeddings per speaker (up to
//! `db::speakers::MAX_EMBEDDINGS_PER_SPEAKER`), not a single running-mean
//! centroid: real inter-session cosine similarity for the same voice
//! measures 0.48 to 0.66 in practice, so any one embedding can miss a match
//! while another recorded on a different day, mic, or vocal tone hits.
//! Similarity against a stored speaker is the MAX over its embeddings.

use super::SpeakerCentroid;

/// Cosine similarity above which a cluster is considered a candidate match
/// for a stored speaker. Calibrated from real inter-session measurements of
/// the same voice (0.48 to 0.66), not from the clustering threshold: a
/// plain 0.65 floor never matched anything in practice.
pub const MATCH_THRESHOLD: f32 = 0.5;

/// Minimum lead a cluster's best-matching stored speaker must have over the
/// second-best DISTINCT stored speaker for the match to be accepted instead
/// of treated as ambiguous. Applies whenever a second stored speaker exists
/// at all, regardless of whether that second speaker's own similarity clears
/// `MATCH_THRESHOLD`: intra-voice similarity varies 0.48 to 0.66 in practice,
/// so a best match of 0.52 against a second-best of 0.47 is well inside that
/// noise band and must not be silently accepted just because 0.47 falls
/// under the threshold. An ambiguous cluster is left unlabeled rather than
/// guessed at: creating a new speaker for it would spawn yet another
/// duplicate of a voice that already has two candidate rows.
pub const MATCH_MARGIN: f32 = 0.05;

/// A cluster below `MATCH_THRESHOLD` only becomes a brand new stored speaker
/// if it has at least this much speech. Guards against a stray fraction of a
/// second of speech spawning a permanent identity that nothing will ever
/// match against again.
pub const MIN_ENROLL_SPEECH_S: f64 = 5.0;

/// A stored, persistent speaker row as seen by the matcher: its recent
/// embeddings (see `db::speakers::MAX_EMBEDDINGS_PER_SPEAKER`), decoded to
/// f32s. Empty for a speaker that has never had an embedding recorded (can't
/// be matched against, only ever loses to every cluster).
#[derive(Debug, Clone, PartialEq)]
pub struct StoredSpeaker {
    pub id: i64,
    pub name: String,
    pub embeddings: Vec<Vec<f32>>,
}

/// What to do with one cluster from a `diarize()` result, decided by
/// `match_speakers`.
#[derive(Debug, Clone, PartialEq)]
pub enum MatchOutcome {
    /// This cluster resolved to an existing stored speaker.
    Matched { speaker_id: i64 },
    /// This cluster did not match anything, and had enough speech to be
    /// worth enrolling as a brand new speaker with this name.
    NewSpeaker { name: String },
    /// This cluster is left without a persistent speaker: either its best
    /// match was ambiguous against two distinct stored speakers, it lost a
    /// same-target conflict to a higher-similarity cluster, or it was below
    /// threshold with too little speech to enroll. Its spans stay unlabeled;
    /// the meeting can still be retagged manually later.
    Unlabeled,
}

/// A `match_speakers` decision for one cluster: which cluster
/// (`DiarizedSegment.speaker` / `SpeakerCentroid.speaker`), what to do
/// (`outcome`), and the embedding and speech duration the caller should
/// persist for `Matched`/`NewSpeaker` (irrelevant for `Unlabeled`, but
/// always populated so the caller doesn't need to special-case it).
#[derive(Debug, Clone, PartialEq)]
pub struct MatchDecision {
    pub cluster: usize,
    pub outcome: MatchOutcome,
    pub embedding: Vec<f32>,
    pub speech_seconds: f64,
}

/// Cosine similarity between two vectors. Returns 0.0 for a zero-length
/// vector or a dimension mismatch rather than panicking or producing NaN.
/// Both are "no evidence of similarity" in this context.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)).clamp(-1.0, 1.0)
}

/// MAX cosine similarity of `embedding` against every embedding in `bag`.
/// `None` for an empty bag: a stored speaker that has never had an embedding
/// recorded has nothing to compare against. Shared by `match_speakers` and
/// the `--diarize-file` CLI calibration report, which both need "how close
/// is this embedding to this speaker's bag of recent voice samples".
pub fn max_similarity(embedding: &[f32], bag: &[Vec<f32>]) -> Option<f32> {
    bag.iter().map(|e| cosine_similarity(embedding, e)).max_by(f32::total_cmp)
}

/// Encode an embedding as the DB's opaque BLOB format: little-endian f32s,
/// one per dimension (256 for the WeSpeaker model currently in use, but
/// nothing here hard-codes that beyond this comment).
pub fn encode_embedding(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// Decode an embedding BLOB back into f32s. `None` if the byte length isn't
/// a multiple of 4 (corrupt or foreign data). Callers treat that exactly
/// like "no embedding", never as a hard error.
pub fn decode_embedding(bytes: &[u8]) -> Option<Vec<f32>> {
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    Some(
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect(),
    )
}

/// The next unused "Speaker N" name, given every already-taken name (stored
/// rows plus any already assigned earlier in this same batch). Scans for the
/// highest `N` in "Speaker <N>" among `taken_names` and returns `N + 1`, so a
/// later rename of an existing speaker can never collide with a fresh one.
fn next_speaker_name<'a>(taken_names: impl Iterator<Item = &'a str>) -> String {
    let max_n = taken_names
        .filter_map(|name| name.strip_prefix("Speaker "))
        .filter_map(|rest| rest.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("Speaker {}", max_n + 1)
}

/// Match every cluster in a `diarize()` result against `stored` speakers,
/// deciding for each cluster whether it's an existing speaker, a brand new
/// one, or left unlabeled.
///
/// For a cluster, similarity against a stored speaker is the MAX cosine
/// similarity over that speaker's embeddings. The best-matching stored
/// speaker is accepted only if `best >= MATCH_THRESHOLD` AND there is no
/// second DISTINCT stored speaker within `MATCH_MARGIN` of it, whether or
/// not that second speaker itself clears `MATCH_THRESHOLD` (an ambiguous
/// best match never creates a duplicate: it's left unlabeled for the user to
/// resolve). Below threshold, the
/// cluster becomes a new speaker only if it has at least `MIN_ENROLL_SPEECH_S`
/// of speech; otherwise it's left unlabeled. Because two different clusters
/// can end up wanting the same stored speaker, accepted candidates are
/// resolved greedily by descending similarity: the higher-similarity
/// cluster keeps the match, the loser is left unlabeled (never demoted to a
/// new speaker: that would still create a duplicate of a voice already
/// matched this pass).
pub fn match_speakers(clusters: &[SpeakerCentroid], stored: &[StoredSpeaker]) -> Vec<MatchDecision> {
    struct Candidate {
        cluster: usize,
        stored_idx: usize,
        similarity: f32,
    }

    enum Prelim {
        Candidate { stored_idx: usize, similarity: f32 },
        Ambiguous,
        BelowThreshold,
    }

    let prelims: Vec<Prelim> = clusters
        .iter()
        .map(|cluster| {
            let mut sims: Vec<(usize, f32)> = stored
                .iter()
                .enumerate()
                .filter_map(|(idx, s)| {
                    max_similarity(&cluster.embedding, &s.embeddings).map(|max_sim| (idx, max_sim))
                })
                .collect();
            sims.sort_by(|a, b| b.1.total_cmp(&a.1));

            match sims.first() {
                Some(&(best_idx, best_sim)) if best_sim >= MATCH_THRESHOLD => {
                    let ambiguous = sims
                        .get(1)
                        .is_some_and(|&(_, second_sim)| best_sim - second_sim < MATCH_MARGIN);
                    if ambiguous {
                        Prelim::Ambiguous
                    } else {
                        Prelim::Candidate { stored_idx: best_idx, similarity: best_sim }
                    }
                }
                _ => Prelim::BelowThreshold,
            }
        })
        .collect();

    let mut candidates: Vec<Candidate> = clusters
        .iter()
        .zip(prelims.iter())
        .filter_map(|(cluster, prelim)| match prelim {
            Prelim::Candidate { stored_idx, similarity } => Some(Candidate {
                cluster: cluster.speaker,
                stored_idx: *stored_idx,
                similarity: *similarity,
            }),
            _ => None,
        })
        .collect();
    candidates.sort_by(|a, b| b.similarity.total_cmp(&a.similarity));

    let mut claimed_stored: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut cluster_match: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for candidate in &candidates {
        if claimed_stored.contains(&candidate.stored_idx) || cluster_match.contains_key(&candidate.cluster) {
            continue;
        }
        claimed_stored.insert(candidate.stored_idx);
        cluster_match.insert(candidate.cluster, candidate.stored_idx);
    }

    let mut taken_names: Vec<String> = stored.iter().map(|s| s.name.clone()).collect();
    let mut decisions = Vec::with_capacity(clusters.len());
    for (cluster, prelim) in clusters.iter().zip(prelims.iter()) {
        let outcome = if let Some(&stored_idx) = cluster_match.get(&cluster.speaker) {
            MatchOutcome::Matched { speaker_id: stored[stored_idx].id }
        } else {
            match prelim {
                Prelim::BelowThreshold if cluster.speech_seconds >= MIN_ENROLL_SPEECH_S => {
                    let name = next_speaker_name(taken_names.iter().map(|s| s.as_str()));
                    taken_names.push(name.clone());
                    MatchOutcome::NewSpeaker { name }
                }
                // Ambiguous, below threshold with too little speech, or lost
                // the greedy same-target conflict: none of these ever create
                // a new speaker, only an already-matched cluster does.
                _ => MatchOutcome::Unlabeled,
            }
        };
        decisions.push(MatchDecision {
            cluster: cluster.speaker,
            outcome,
            embedding: cluster.embedding.clone(),
            speech_seconds: cluster.speech_seconds,
        });
    }
    decisions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(dim: usize, hot: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; dim];
        v[hot] = 1.0;
        v
    }

    fn normalize(mut v: Vec<f32>) -> Vec<f32> {
        crate::diarize::clustering::l2_normalize(&mut v);
        v
    }

    fn centroid(speaker: usize, embedding: Vec<f32>, speech_seconds: f64) -> SpeakerCentroid {
        SpeakerCentroid { speaker, embedding, speech_seconds }
    }

    fn stored(id: i64, name: &str, embeddings: Vec<Vec<f32>>) -> StoredSpeaker {
        StoredSpeaker { id, name: name.to_string(), embeddings }
    }

    const LONG: f64 = 10.0; // well above MIN_ENROLL_SPEECH_S
    const SHORT: f64 = 1.0; // well below MIN_ENROLL_SPEECH_S

    #[test]
    fn cosine_similarity_identical_vectors_is_one() {
        let v = unit(4, 0);
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_orthogonal_vectors_is_zero() {
        assert!((cosine_similarity(&unit(4, 0), &unit(4, 1))).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_opposite_vectors_is_negative_one() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_handles_zero_vector() {
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn cosine_similarity_handles_dimension_mismatch() {
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[1.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn cosine_similarity_handles_empty_vectors() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }

    #[test]
    fn max_similarity_picks_the_best_of_the_bag() {
        let far = unit(4, 1);
        let close = unit(4, 0);
        let sim = max_similarity(&unit(4, 0), &[far, close]).unwrap();
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn max_similarity_empty_bag_is_none() {
        assert!(max_similarity(&unit(4, 0), &[]).is_none());
    }

    #[test]
    fn embedding_blob_round_trips() {
        let v = vec![0.1f32, -0.2, 0.3, 1.0, -1.0];
        let bytes = encode_embedding(&v);
        assert_eq!(bytes.len(), v.len() * 4);
        let decoded = decode_embedding(&bytes).expect("decode");
        for (a, b) in v.iter().zip(decoded.iter()) {
            assert!((a - b).abs() < 1e-9);
        }
    }

    #[test]
    fn decode_embedding_rejects_truncated_bytes() {
        assert!(decode_embedding(&[0, 1, 2]).is_none());
    }

    #[test]
    fn next_speaker_name_starts_at_one_with_no_existing_speakers() {
        assert_eq!(next_speaker_name(std::iter::empty()), "Speaker 1");
    }

    #[test]
    fn next_speaker_name_increments_past_the_highest_numbered() {
        assert_eq!(
            next_speaker_name(["Speaker 1", "Speaker 3", "Speaker 2"].into_iter()),
            "Speaker 4"
        );
    }

    #[test]
    fn next_speaker_name_ignores_unrelated_names() {
        // A renamed speaker ("Alice") must not reset the counter, and must
        // not be misparsed as a numbered slot either.
        assert_eq!(
            next_speaker_name(["Alice", "Speaker 5", "Bob"].into_iter()),
            "Speaker 6"
        );
    }

    #[test]
    fn next_speaker_name_ignores_malformed_numbers() {
        assert_eq!(
            next_speaker_name(["Speaker abc", "Speaker 2x", "Speaker 7"].into_iter()),
            "Speaker 8"
        );
    }

    #[test]
    fn match_speakers_empty_stored_creates_new_speakers_for_every_cluster() {
        let clusters = vec![centroid(0, unit(4, 0), LONG), centroid(1, unit(4, 1), LONG)];
        let decisions = match_speakers(&clusters, &[]);
        assert_eq!(decisions.len(), 2);
        let names: Vec<&str> = decisions
            .iter()
            .map(|d| match &d.outcome {
                MatchOutcome::NewSpeaker { name } => name.as_str(),
                other => panic!("expected NewSpeaker, got {other:?}"),
            })
            .collect();
        assert_eq!(names, vec!["Speaker 1", "Speaker 2"]);
    }

    #[test]
    fn match_speakers_above_threshold_and_margin_matches_existing_speaker() {
        let clusters = vec![centroid(0, unit(4, 0), SHORT)];
        let stored_speakers = vec![stored(10, "Alice", vec![unit(4, 0)]), stored(11, "Bob", vec![unit(4, 1)])];
        let decisions = match_speakers(&clusters, &stored_speakers);
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].outcome, MatchOutcome::Matched { speaker_id: 10 });
    }

    #[test]
    fn match_speakers_below_threshold_and_long_speech_creates_new_speaker() {
        let clusters = vec![centroid(0, unit(4, 1), LONG)];
        let stored_speakers = vec![stored(10, "Alice", vec![unit(4, 0)])]; // sim 0.0
        let decisions = match_speakers(&clusters, &stored_speakers);
        assert_eq!(
            decisions[0].outcome,
            MatchOutcome::NewSpeaker { name: "Speaker 1".to_string() }
        );
    }

    #[test]
    fn match_speakers_below_threshold_and_short_speech_is_unlabeled() {
        let clusters = vec![centroid(0, unit(4, 1), SHORT)];
        let stored_speakers = vec![stored(10, "Alice", vec![unit(4, 0)])]; // sim 0.0
        let decisions = match_speakers(&clusters, &stored_speakers);
        assert_eq!(
            decisions[0].outcome,
            MatchOutcome::Unlabeled,
            "too little speech below threshold must not enroll a new identity"
        );
    }

    #[test]
    fn match_speakers_ambiguous_margin_is_unlabeled_even_with_long_speech() {
        // Two stored speakers whose embeddings are both close to the cluster
        // and close to each other: best and second-best land within the
        // margin, so the match must be rejected as ambiguous rather than
        // spawning a third duplicate.
        let a = normalize(vec![1.0, 0.05, 0.0, 0.0]);
        let b = normalize(vec![1.0, -0.05, 0.0, 0.0]);
        let cluster_embedding = normalize(vec![1.0, 0.0, 0.0, 0.0]);
        let clusters = vec![centroid(0, cluster_embedding, LONG)];
        let stored_speakers = vec![stored(10, "Alice", vec![a]), stored(11, "Bob", vec![b])];
        let decisions = match_speakers(&clusters, &stored_speakers);
        assert_eq!(
            decisions[0].outcome,
            MatchOutcome::Unlabeled,
            "ambiguous match must never fall back to creating a new speaker"
        );
    }

    #[test]
    fn match_speakers_ambiguous_when_second_is_below_threshold_but_within_margin() {
        // Best match 0.52 against Alice, second-best 0.47 against Bob: 0.47
        // is BELOW MATCH_THRESHOLD (0.5), but the 0.05 gap is inside the
        // documented intra-voice variance (0.48-0.66), so this must still be
        // ambiguous, not silently matched to Alice.
        let cluster_embedding = vec![1.0, 0.0, 0.0, 0.0];
        let a = vec![0.52, (1.0f32 - 0.52 * 0.52).sqrt(), 0.0, 0.0];
        let b = vec![0.47, 0.0, (1.0f32 - 0.47 * 0.47).sqrt(), 0.0];
        let clusters = vec![centroid(0, cluster_embedding, LONG)];
        let stored_speakers = vec![stored(10, "Alice", vec![a]), stored(11, "Bob", vec![b])];
        let decisions = match_speakers(&clusters, &stored_speakers);
        assert_eq!(
            decisions[0].outcome,
            MatchOutcome::Unlabeled,
            "a second-best within MATCH_MARGIN must be ambiguous even when itself below MATCH_THRESHOLD"
        );
    }

    #[test]
    fn match_speakers_speaker_without_embeddings_never_matches() {
        let clusters = vec![centroid(0, unit(4, 0), LONG)];
        let stored_speakers = vec![stored(10, "Alice", Vec::new())];
        let decisions = match_speakers(&clusters, &stored_speakers);
        assert_eq!(
            decisions[0].outcome,
            MatchOutcome::NewSpeaker { name: "Speaker 1".to_string() }
        );
    }

    #[test]
    fn match_speakers_max_similarity_across_multiple_embeddings_matches() {
        // Alice has one far (orthogonal) embedding and one close (identical
        // to the cluster) one. The match must succeed via the MAX, not fail
        // because of the far one.
        let far = unit(4, 1);
        let close = unit(4, 0);
        let clusters = vec![centroid(0, unit(4, 0), SHORT)];
        let stored_speakers = vec![stored(10, "Alice", vec![far, close])];
        let decisions = match_speakers(&clusters, &stored_speakers);
        assert_eq!(decisions[0].outcome, MatchOutcome::Matched { speaker_id: 10 });
    }

    #[test]
    fn match_speakers_two_clusters_never_map_to_the_same_stored_speaker() {
        // Both clusters are near-identical to the single stored speaker; the
        // higher-similarity one (cluster 0, exact match) must win, the
        // other must be left unlabeled rather than double-mapping or
        // spawning a new speaker (even though it has plenty of speech).
        let stored_speakers = vec![stored(10, "Alice", vec![unit(4, 0)])];
        let almost = normalize(vec![1.0, 0.2, 0.0, 0.0]);
        let clusters = vec![centroid(0, unit(4, 0), LONG), centroid(1, almost, LONG)];
        let decisions = match_speakers(&clusters, &stored_speakers);

        let d0 = decisions.iter().find(|d| d.cluster == 0).unwrap();
        let d1 = decisions.iter().find(|d| d.cluster == 1).unwrap();
        assert_eq!(d0.outcome, MatchOutcome::Matched { speaker_id: 10 });
        assert_eq!(
            d1.outcome,
            MatchOutcome::Unlabeled,
            "loser of a same-target conflict must be left unlabeled, not double-mapped or re-enrolled"
        );
    }

    #[test]
    fn match_speakers_multiple_new_speakers_get_sequential_distinct_names() {
        let clusters = vec![
            centroid(0, unit(3, 0), LONG),
            centroid(1, unit(3, 1), LONG),
            centroid(2, unit(3, 2), LONG),
        ];
        let stored_speakers = vec![stored(1, "Speaker 2", Vec::new())];
        let decisions = match_speakers(&clusters, &stored_speakers);
        let names: Vec<&str> = decisions
            .iter()
            .map(|d| match &d.outcome {
                MatchOutcome::NewSpeaker { name } => name.as_str(),
                other => panic!("expected NewSpeaker, got {other:?}"),
            })
            .collect();
        assert_eq!(names, vec!["Speaker 3", "Speaker 4", "Speaker 5"]);
    }

    #[test]
    fn match_speakers_empty_clusters_returns_empty_decisions() {
        let stored_speakers = vec![stored(10, "Alice", vec![unit(4, 0)])];
        assert!(match_speakers(&[], &stored_speakers).is_empty());
    }

    #[test]
    fn match_speakers_carries_embedding_and_speech_seconds_on_every_decision() {
        let embedding = unit(4, 0);
        let clusters = vec![centroid(0, embedding.clone(), 7.5)];
        let decisions = match_speakers(&clusters, &[]);
        assert_eq!(decisions[0].embedding, embedding);
        assert_eq!(decisions[0].speech_seconds, 7.5);
    }
}
