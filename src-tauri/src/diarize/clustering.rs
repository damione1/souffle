//! Cosine-similarity greedy agglomerative clustering over speaker embeddings.
//!
//! Pure math, no ONNX dependency: given one embedding per candidate speech
//! segment, repeatedly merges the two most similar clusters (average-linkage
//! via a running L2-normalized centroid) until either the best remaining
//! similarity drops below `threshold` or `min_speakers` is reached. If
//! `max_speakers` is set, merging continues past the threshold until the
//! cluster count is at most `max_speakers`.

/// L2-normalize a vector in place. A zero vector is left unchanged (cosine
/// similarity against it is defined as 0 by `cosine_similarity` below).
pub fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-12 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Cosine similarity between two vectors of equal length.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a <= 1e-12 || norm_b <= 1e-12 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// Result of clustering a set of embeddings: one cluster label per input
/// embedding (in input order), and one L2-normalized centroid per cluster
/// (indexed by label).
pub struct ClusterResult {
    pub labels: Vec<usize>,
    pub centroids: Vec<Vec<f32>>,
}

/// Greedy agglomerative clustering by cosine similarity.
///
/// `min_speakers`/`max_speakers` bound the final cluster count when given.
/// With neither set, merging stops naturally once the best remaining
/// pairwise similarity falls below `threshold`.
pub fn cluster(
    embeddings: &[Vec<f32>],
    threshold: f32,
    min_speakers: Option<usize>,
    max_speakers: Option<usize>,
) -> ClusterResult {
    let n = embeddings.len();
    if n == 0 {
        return ClusterResult {
            labels: Vec::new(),
            centroids: Vec::new(),
        };
    }

    let dim = embeddings[0].len();
    let mut members: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
    let mut centroids: Vec<Vec<f32>> = embeddings
        .iter()
        .map(|e| {
            let mut v = e.clone();
            l2_normalize(&mut v);
            v
        })
        .collect();

    let min_k = min_speakers.unwrap_or(1).max(1);
    let max_k = max_speakers.unwrap_or(n).max(min_k);

    loop {
        let k = members.len();
        if k <= min_k {
            break;
        }

        let mut best = (0usize, 0usize, f32::MIN);
        for i in 0..k {
            for j in (i + 1)..k {
                let sim = cosine_similarity(&centroids[i], &centroids[j]);
                if sim > best.2 {
                    best = (i, j, sim);
                }
            }
        }
        let (i, j, sim) = best;

        let must_reduce = k > max_k;
        if !must_reduce && sim < threshold {
            break;
        }

        let absorbed = members.remove(j);
        members[i].extend(absorbed);
        centroids[i] = centroid_of(embeddings, &members[i], dim);
        centroids.remove(j);
    }

    let mut labels = vec![0usize; n];
    for (cluster_id, member_indexes) in members.iter().enumerate() {
        for &idx in member_indexes {
            labels[idx] = cluster_id;
        }
    }

    ClusterResult { labels, centroids }
}

/// Mean of the L2-normalized member embeddings, re-normalized. Matches the
/// running centroid update used during clustering, so a cluster's final
/// centroid is consistent with the similarity scores that produced it.
fn centroid_of(embeddings: &[Vec<f32>], member_indexes: &[usize], dim: usize) -> Vec<f32> {
    let mut mean = vec![0.0f32; dim];
    for &idx in member_indexes {
        let mut v = embeddings[idx].clone();
        l2_normalize(&mut v);
        for (m, x) in mean.iter_mut().zip(v.iter()) {
            *m += x;
        }
    }
    l2_normalize(&mut mean);
    mean
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(mut v: Vec<f32>) -> Vec<f32> {
        l2_normalize(&mut v);
        v
    }

    #[test]
    fn cosine_similarity_identical_vectors_is_one() {
        let a = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_orthogonal_vectors_is_zero() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_opposite_vectors_is_minus_one() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_zero_vector_is_zero() {
        let a = vec![0.0, 0.0];
        let b = vec![1.0, 0.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    /// Two tight synthetic clusters (near-identical vectors within a group,
    /// clearly different between groups) must split into exactly two labels.
    #[test]
    fn cluster_splits_two_well_separated_groups() {
        let embeddings = vec![
            unit(vec![1.0, 0.0, 0.0]),
            unit(vec![0.98, 0.02, 0.0]),
            unit(vec![0.99, 0.01, 0.0]),
            unit(vec![0.0, 1.0, 0.0]),
            unit(vec![0.02, 0.98, 0.0]),
            unit(vec![0.01, 0.99, 0.0]),
        ];
        let result = cluster(&embeddings, 0.7, None, None);
        assert_eq!(result.labels[0], result.labels[1]);
        assert_eq!(result.labels[1], result.labels[2]);
        assert_eq!(result.labels[3], result.labels[4]);
        assert_eq!(result.labels[4], result.labels[5]);
        assert_ne!(result.labels[0], result.labels[3]);
        assert_eq!(result.centroids.len(), 2);
    }

    #[test]
    fn cluster_single_embedding_yields_one_cluster() {
        let embeddings = vec![unit(vec![1.0, 0.0])];
        let result = cluster(&embeddings, 0.7, None, None);
        assert_eq!(result.labels, vec![0]);
        assert_eq!(result.centroids.len(), 1);
    }

    #[test]
    fn cluster_empty_input_yields_nothing() {
        let result = cluster(&[], 0.7, None, None);
        assert!(result.labels.is_empty());
        assert!(result.centroids.is_empty());
    }

    /// Two identical vectors would normally merge under any real threshold;
    /// `min_speakers` should force them to stay apart.
    #[test]
    fn cluster_min_speakers_prevents_merging_below_floor() {
        let embeddings = vec![unit(vec![1.0, 0.0]), unit(vec![1.0, 0.0001])];
        let result = cluster(&embeddings, 0.99, Some(2), None);
        assert_ne!(result.labels[0], result.labels[1]);
        assert_eq!(result.centroids.len(), 2);
    }

    /// Two very dissimilar vectors would normally stay apart under a high
    /// threshold; `max_speakers` should force a merge anyway.
    #[test]
    fn cluster_max_speakers_forces_merge_above_ceiling() {
        let embeddings = vec![
            unit(vec![1.0, 0.0, 0.0]),
            unit(vec![0.0, 1.0, 0.0]),
            unit(vec![0.0, 0.0, 1.0]),
        ];
        let result = cluster(&embeddings, 0.99, None, Some(1));
        assert_eq!(result.centroids.len(), 1);
        assert!(result.labels.iter().all(|&l| l == result.labels[0]));
    }

    #[test]
    fn cluster_centroids_are_unit_length() {
        let embeddings = vec![unit(vec![1.0, 0.0]), unit(vec![0.9, 0.1])];
        let result = cluster(&embeddings, 0.5, None, None);
        for c in &result.centroids {
            let norm: f32 = c.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 1e-5 || norm < 1e-5);
        }
    }
}
