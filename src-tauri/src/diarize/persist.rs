//! Matches a `diarize()` run's per-meeting speaker clusters against
//! persistent, cross-meeting speaker identities stored in the `speakers`
//! table, so the same voice resolves to the same speaker row across
//! meetings. Pure decision logic only: no I/O, no database access. The
//! caller (`pipeline::diarize_task`) is responsible for turning the returned
//! `MatchDecision`s into `create_speaker`/`update_speaker_centroid` calls.

use super::SpeakerCentroid;

/// Cosine similarity above which a cluster is considered a candidate match
/// for a stored speaker.
pub const MATCH_THRESHOLD: f32 = 0.65;

/// Minimum lead a cluster's best-matching stored speaker must have over the
/// second-best, among all OTHER stored speakers, for the match to be
/// accepted instead of treated as ambiguous (and so mapped to a new speaker).
pub const MATCH_MARGIN: f32 = 0.03;

/// A stored, persistent speaker row as seen by the matcher. `centroid` is
/// `None` for a speaker that has never had an embedding recorded (can't be
/// matched against, only ever loses to every cluster).
#[derive(Debug, Clone, PartialEq)]
pub struct StoredSpeaker {
    pub id: i64,
    pub name: String,
    pub centroid: Option<Vec<f32>>,
    pub embedding_count: i64,
}

/// What to do with one cluster from a `diarize()` result, decided by
/// `match_speakers`. Exactly one of `matched_speaker_id`/`new_name` is set.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchDecision {
    /// Index into the `DiarizationResult.speakers` slice this decision
    /// covers (also the local speaker id used in `DiarizedSegment.speaker`).
    pub cluster: usize,
    /// `Some(id)`: this cluster resolved to an existing stored speaker.
    pub matched_speaker_id: Option<i64>,
    /// `Some(name)`: this cluster did not match anything and should become a
    /// brand new speaker with this name. Only set when `matched_speaker_id`
    /// is `None`.
    pub new_name: Option<String>,
    /// The centroid to persist for the resolved speaker: a running mean of
    /// the old centroid (if any) and this cluster's embedding, re-normalized.
    pub updated_centroid: Vec<f32>,
    /// The `embedding_count` to persist alongside `updated_centroid`.
    pub updated_embedding_count: i64,
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

/// Encode a centroid as the DB's opaque BLOB format: 256 little-endian f32s
/// (or however many dimensions `v` has; the embedding model's output size
/// is fixed, but nothing here hard-codes 256 beyond the doc comment).
pub fn encode_centroid(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// Decode a centroid BLOB back into f32s. `None` if the byte length isn't a
/// multiple of 4 (corrupt or foreign data). Callers treat that exactly like
/// "no centroid yet", never as a hard error.
pub fn decode_centroid(bytes: &[u8]) -> Option<Vec<f32>> {
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

/// Re-normalize `v` to unit length. A zero vector is returned unchanged
/// (nothing sensible to normalize it to).
fn l2_normalize(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

/// Running-mean centroid update: `old * old_count + new`, re-normalized.
/// `old` is `None` for a brand new speaker, in which case the result is just
/// `new` (already L2-normalized by the clustering stage) with count 1.
fn update_centroid(old: Option<(&[f32], i64)>, new: &[f32]) -> (Vec<f32>, i64) {
    match old {
        None => (new.to_vec(), 1),
        Some((old_centroid, old_count)) if old_centroid.len() == new.len() => {
            let weight = old_count.max(0) as f32;
            let combined: Vec<f32> = old_centroid
                .iter()
                .zip(new.iter())
                .map(|(o, n)| o * weight + n)
                .collect();
            (l2_normalize(combined), old_count + 1)
        }
        // Dimension mismatch (shouldn't happen in practice; the embedding
        // model's output size is fixed, but a corrupt/foreign BLOB must not
        // panic): treat as if there were no prior centroid.
        Some(_) => (new.to_vec(), 1),
    }
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
/// deciding for each cluster whether it's an existing speaker (and its
/// updated centroid) or a brand new one.
///
/// Matching rule: for a cluster, compute cosine similarity against every
/// stored speaker that has a centroid. The best match is accepted only if
/// `best >= MATCH_THRESHOLD` AND `best - second_best >= MATCH_MARGIN` (an
/// ambiguous best match, or a match below the confidence floor, both fall
/// back to "new speaker"). Because two different clusters can end up wanting
/// the same stored speaker, candidates are resolved greedily by descending
/// similarity: the higher-similarity cluster keeps the match, the loser
/// becomes a new speaker instead.
pub fn match_speakers(clusters: &[SpeakerCentroid], stored: &[StoredSpeaker]) -> Vec<MatchDecision> {
    struct Candidate {
        cluster: usize,
        stored_idx: usize,
        similarity: f32,
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    for cluster in clusters {
        let mut sims: Vec<(usize, f32)> = stored
            .iter()
            .enumerate()
            .filter_map(|(idx, s)| {
                s.centroid
                    .as_ref()
                    .map(|c| (idx, cosine_similarity(&cluster.embedding, c)))
            })
            .collect();
        sims.sort_by(|a, b| b.1.total_cmp(&a.1));

        let Some(&(best_idx, best_sim)) = sims.first() else {
            continue;
        };
        let second_sim = sims.get(1).map(|&(_, s)| s).unwrap_or(f32::MIN);
        if best_sim >= MATCH_THRESHOLD && best_sim - second_sim >= MATCH_MARGIN {
            candidates.push(Candidate {
                cluster: cluster.speaker,
                stored_idx: best_idx,
                similarity: best_sim,
            });
        }
    }
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
    for cluster in clusters {
        match cluster_match.get(&cluster.speaker) {
            Some(&stored_idx) => {
                let stored_speaker = &stored[stored_idx];
                let (updated_centroid, updated_embedding_count) = update_centroid(
                    stored_speaker
                        .centroid
                        .as_deref()
                        .map(|c| (c, stored_speaker.embedding_count)),
                    &cluster.embedding,
                );
                decisions.push(MatchDecision {
                    cluster: cluster.speaker,
                    matched_speaker_id: Some(stored_speaker.id),
                    new_name: None,
                    updated_centroid,
                    updated_embedding_count,
                });
            }
            None => {
                let name = next_speaker_name(taken_names.iter().map(|s| s.as_str()));
                taken_names.push(name.clone());
                let (updated_centroid, updated_embedding_count) = update_centroid(None, &cluster.embedding);
                decisions.push(MatchDecision {
                    cluster: cluster.speaker,
                    matched_speaker_id: None,
                    new_name: Some(name),
                    updated_centroid,
                    updated_embedding_count,
                });
            }
        }
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

    fn centroid(speaker: usize, embedding: Vec<f32>) -> SpeakerCentroid {
        SpeakerCentroid { speaker, embedding }
    }

    fn stored(id: i64, name: &str, centroid: Option<Vec<f32>>, embedding_count: i64) -> StoredSpeaker {
        StoredSpeaker {
            id,
            name: name.to_string(),
            centroid,
            embedding_count,
        }
    }

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
    fn centroid_blob_round_trips() {
        let v = vec![0.1f32, -0.2, 0.3, 1.0, -1.0];
        let bytes = encode_centroid(&v);
        assert_eq!(bytes.len(), v.len() * 4);
        let decoded = decode_centroid(&bytes).expect("decode");
        for (a, b) in v.iter().zip(decoded.iter()) {
            assert!((a - b).abs() < 1e-9);
        }
    }

    #[test]
    fn decode_centroid_rejects_truncated_bytes() {
        assert!(decode_centroid(&[0, 1, 2]).is_none());
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
        let clusters = vec![centroid(0, unit(4, 0)), centroid(1, unit(4, 1))];
        let decisions = match_speakers(&clusters, &[]);
        assert_eq!(decisions.len(), 2);
        assert!(decisions.iter().all(|d| d.matched_speaker_id.is_none()));
        let names: Vec<&str> = decisions.iter().map(|d| d.new_name.as_deref().unwrap()).collect();
        assert_eq!(names, vec!["Speaker 1", "Speaker 2"]);
        for d in &decisions {
            assert_eq!(d.updated_embedding_count, 1);
        }
    }

    #[test]
    fn match_speakers_above_threshold_and_margin_matches_existing_speaker() {
        let clusters = vec![centroid(0, unit(4, 0))];
        let stored_speakers = vec![
            stored(10, "Alice", Some(unit(4, 0)), 3),
            stored(11, "Bob", Some(unit(4, 1)), 3),
        ];
        let decisions = match_speakers(&clusters, &stored_speakers);
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].matched_speaker_id, Some(10));
        assert_eq!(decisions[0].new_name, None);
        assert_eq!(decisions[0].updated_embedding_count, 4);
    }

    #[test]
    fn match_speakers_below_threshold_creates_new_speaker() {
        // 45-degree vector is ~0.707 similar to each axis, comfortably above
        // 0.65, so use something further off-axis to stay under threshold.
        let low_sim = vec![0.5f32, 0.0, 0.0, (1.0f32 - 0.25).sqrt()];
        let clusters = vec![centroid(0, low_sim)];
        let stored_speakers = vec![stored(10, "Alice", Some(unit(4, 0)), 1)];
        let decisions = match_speakers(&clusters, &stored_speakers);
        assert_eq!(decisions[0].matched_speaker_id, None);
        assert_eq!(decisions[0].new_name.as_deref(), Some("Speaker 1"));
    }

    #[test]
    fn match_speakers_ambiguous_margin_creates_new_speaker() {
        // Two stored speakers whose centroids are both close to the cluster
        // and close to each other: best and second-best land within the
        // margin, so the match must be rejected as ambiguous.
        let a = l2_normalize(vec![1.0, 0.05, 0.0, 0.0]);
        let b = l2_normalize(vec![1.0, -0.05, 0.0, 0.0]);
        let cluster_embedding = l2_normalize(vec![1.0, 0.0, 0.0, 0.0]);
        let clusters = vec![centroid(0, cluster_embedding)];
        let stored_speakers = vec![stored(10, "Alice", Some(a), 1), stored(11, "Bob", Some(b), 1)];
        let decisions = match_speakers(&clusters, &stored_speakers);
        assert_eq!(decisions[0].matched_speaker_id, None, "ambiguous match must fall back to new speaker");
    }

    #[test]
    fn match_speakers_speaker_without_centroid_is_never_matched() {
        let clusters = vec![centroid(0, unit(4, 0))];
        let stored_speakers = vec![stored(10, "Alice", None, 0)];
        let decisions = match_speakers(&clusters, &stored_speakers);
        assert_eq!(decisions[0].matched_speaker_id, None);
        assert_eq!(decisions[0].new_name.as_deref(), Some("Speaker 1"));
    }

    #[test]
    fn match_speakers_two_clusters_never_map_to_the_same_stored_speaker() {
        // Both clusters are near-identical to the single stored speaker; the
        // higher-similarity one (cluster 0, exact match) must win, the
        // other must become a new speaker rather than double-mapping.
        let stored_speakers = vec![stored(10, "Alice", Some(unit(4, 0)), 5)];
        let almost = l2_normalize(vec![1.0, 0.2, 0.0, 0.0]);
        let clusters = vec![centroid(0, unit(4, 0)), centroid(1, almost)];
        let decisions = match_speakers(&clusters, &stored_speakers);

        let d0 = decisions.iter().find(|d| d.cluster == 0).unwrap();
        let d1 = decisions.iter().find(|d| d.cluster == 1).unwrap();
        assert_eq!(d0.matched_speaker_id, Some(10));
        assert_eq!(d1.matched_speaker_id, None, "loser must not double-map to the same stored speaker");
        assert_eq!(d1.new_name.as_deref(), Some("Speaker 1"));
    }

    #[test]
    fn match_speakers_multiple_new_speakers_get_sequential_distinct_names() {
        let clusters = vec![
            centroid(0, unit(3, 0)),
            centroid(1, unit(3, 1)),
            centroid(2, unit(3, 2)),
        ];
        let stored_speakers = vec![stored(1, "Speaker 2", None, 0)];
        let decisions = match_speakers(&clusters, &stored_speakers);
        let names: Vec<&str> = decisions.iter().map(|d| d.new_name.as_deref().unwrap()).collect();
        assert_eq!(names, vec!["Speaker 3", "Speaker 4", "Speaker 5"]);
    }

    #[test]
    fn update_centroid_running_mean_matches_hand_computed_value() {
        // old = [1, 0] with count 2, new = [0, 1] -> combined pre-normalize:
        // [1*2 + 0, 0*2 + 1] = [2, 1], normalized = [2, 1] / sqrt(5).
        let (result, count) = update_centroid(Some((&[1.0, 0.0], 2)), &[0.0, 1.0]);
        let expected_norm = (5.0f32).sqrt();
        assert!((result[0] - 2.0 / expected_norm).abs() < 1e-6);
        assert!((result[1] - 1.0 / expected_norm).abs() < 1e-6);
        assert_eq!(count, 3);
    }

    #[test]
    fn update_centroid_with_no_prior_history_is_just_the_new_embedding() {
        let (result, count) = update_centroid(None, &[0.6, 0.8]);
        assert_eq!(result, vec![0.6, 0.8]);
        assert_eq!(count, 1);
    }

    #[test]
    fn match_speakers_empty_clusters_returns_empty_decisions() {
        let stored_speakers = vec![stored(10, "Alice", Some(unit(4, 0)), 1)];
        assert!(match_speakers(&[], &stored_speakers).is_empty());
    }
}
