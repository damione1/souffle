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
//!
//! `resolve_session` builds on `match_speakers` to coordinate a single
//! recording session's mic and system-audio passes: it detects mic clusters
//! that are just the system audio leaking into the microphone (see its doc
//! comment for the physical invariant that makes this safe) and folds the
//! remaining, genuinely independent clusters from both lanes into one
//! `match_speakers` call, so a claim on a stored speaker can no longer be won
//! twice by two clusters that are actually the same voice heard through two
//! taps.

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

/// Cosine similarity above which a mic cluster is considered the same voice
/// as a system cluster from the SAME session, i.e. the system audio leaking
/// acoustically into the microphone, not two different people who happen to
/// sound alike. Deliberately higher than `MATCH_THRESHOLD` (0.5):
/// `MATCH_THRESHOLD` calibrates the same voice recorded on different
/// days/mics/tones (0.48 to 0.66 in practice), a much noisier comparison
/// than the same voice at the same instant of the same session, only
/// degraded by one extra speaker-to-mic acoustic path.
///
/// The physical invariant that makes this safe: the system audio tap can
/// never contain the local microphone's own voice (there is no return loop
/// from mic input back into system output), so a mic cluster that closely
/// matches a system cluster from the same session cannot be a coincidence of
/// two different people sounding similar. It IS that system voice, heard
/// twice.
pub const CROSS_LANE_ECHO_THRESHOLD: f32 = 0.7;

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

/// What a mic cluster resolved to, once cross-lane echo has been factored
/// in. See `resolve_session`.
#[derive(Debug, Clone, PartialEq)]
pub enum MicResolution {
    /// Not echo (or there was no system lane to compare against): resolved
    /// on its own merits, exactly as `match_speakers` would in isolation.
    Own(MatchDecision),
    /// This mic cluster is the system audio leaking into the microphone,
    /// heard a second time. `cluster` is its own (mic-lane) cluster id;
    /// `system_cluster` is the system-lane cluster id (indexing into
    /// `SessionResolution::system`, matched by `MatchDecision::cluster`) it
    /// is an echo of. It is never matched, enrolled, or persisted on its
    /// own: the caller must resolve it to whatever the system cluster
    /// resolved to (including `Unlabeled`, if the system cluster itself
    /// didn't resolve to a stored speaker) and must never write an embedding
    /// for it, since it is a degraded copy of a voice already captured
    /// cleanly by the system tap.
    Echo { cluster: usize, system_cluster: usize },
}

/// One recording session's mic and system-audio clusters, resolved
/// together. See `resolve_session`.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionResolution {
    /// One decision per system cluster, in no particular order. System
    /// clusters are never echo: the system tap can never contain the local
    /// mic's own voice, so nothing about a system cluster ever depends on
    /// the mic lane.
    pub system: Vec<MatchDecision>,
    /// One resolution per mic cluster, index-aligned with the
    /// `mic_clusters` slice passed to `resolve_session`.
    pub mic: Vec<MicResolution>,
}

/// Resolve one session's system and mic clusters together against `stored`
/// speakers, so a mic cluster that is just the system audio leaking into the
/// microphone can never spawn a duplicate identity or steal a stored
/// speaker's claim out from under the real system cluster.
///
/// Two steps:
/// 1. Every mic cluster is compared against every system cluster of the same
///    session (max cosine similarity, see `max_similarity`'s sibling logic
///    here). A mic cluster whose best system match is at or above
///    `CROSS_LANE_ECHO_THRESHOLD` is echo: it is excluded from matching and
///    instead tagged with the system cluster it echoes.
/// 2. Every system cluster, plus every non-echo mic cluster, is matched
///    together in a single `match_speakers` call, so the greedy same-target
///    conflict resolution there (see its doc comment) now also arbitrates
///    between the two lanes, not just within one.
///
/// Mic and system clusters can carry numerically identical `.speaker` ids
/// (each lane numbers its own clusters from its own `diarize()` run), so
/// non-echo mic clusters are re-tagged with an id past every system id
/// before the combined `match_speakers` call, and untagged again on the way
/// out; `MicResolution`/`MatchDecision::cluster` always carries the real,
/// original per-lane id.
///
/// A meeting with only one lane (mic-only or system-only) behaves exactly as
/// `match_speakers` would on that lane alone: an empty other-lane slice
/// finds no echo and contributes nothing to the combined match.
pub fn resolve_session(
    system_clusters: &[SpeakerCentroid],
    mic_clusters: &[SpeakerCentroid],
    stored: &[StoredSpeaker],
) -> SessionResolution {
    // For each mic cluster (keyed by its real cluster id), the system
    // cluster id it is an echo of, if any.
    let echo_of: std::collections::HashMap<usize, usize> = mic_clusters
        .iter()
        .filter_map(|mic| {
            system_clusters
                .iter()
                .map(|sys| (sys.speaker, cosine_similarity(&mic.embedding, &sys.embedding)))
                .filter(|&(_, sim)| sim >= CROSS_LANE_ECHO_THRESHOLD)
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(sys_speaker, _)| (mic.speaker, sys_speaker))
        })
        .collect();

    let tag_offset = system_clusters.iter().map(|c| c.speaker).max().map_or(0, |m| m + 1);
    let mut combined: Vec<SpeakerCentroid> = system_clusters.to_vec();
    // Maps a tagged (combined-list) id back to the real mic cluster id it
    // stands in for, so the match result can be untagged on the way out.
    let mut tag_to_mic_speaker: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for mic in mic_clusters {
        if echo_of.contains_key(&mic.speaker) {
            continue;
        }
        let tag = tag_offset + mic.speaker;
        tag_to_mic_speaker.insert(tag, mic.speaker);
        combined.push(SpeakerCentroid {
            speaker: tag,
            embedding: mic.embedding.clone(),
            speech_seconds: mic.speech_seconds,
        });
    }

    let mut system: Vec<MatchDecision> = Vec::with_capacity(system_clusters.len());
    let mut mic_own: std::collections::HashMap<usize, MatchDecision> = std::collections::HashMap::new();
    for mut decision in match_speakers(&combined, stored) {
        if let Some(&mic_speaker) = tag_to_mic_speaker.get(&decision.cluster) {
            decision.cluster = mic_speaker;
            mic_own.insert(mic_speaker, decision);
        } else {
            system.push(decision);
        }
    }

    let mic = mic_clusters
        .iter()
        .map(|c| match echo_of.get(&c.speaker) {
            Some(&system_cluster) => MicResolution::Echo { cluster: c.speaker, system_cluster },
            None => MicResolution::Own(
                mic_own
                    .remove(&c.speaker)
                    .expect("every non-echo mic cluster was included in the combined match_speakers call"),
            ),
        })
        .collect();

    SessionResolution { system, mic }
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

    // resolve_session

    #[test]
    fn resolve_session_near_identical_mic_cluster_is_echo_of_system_cluster() {
        let sys_emb = unit(4, 0);
        let mic_emb = normalize(vec![1.0, 0.1, 0.0, 0.0]); // sim ~0.995, well above 0.7
        let system_clusters = vec![centroid(0, sys_emb, LONG)];
        let mic_clusters = vec![centroid(0, mic_emb, LONG)];
        let resolution = resolve_session(&system_clusters, &mic_clusters, &[]);

        assert_eq!(resolution.system.len(), 1);
        assert_eq!(resolution.mic, vec![MicResolution::Echo { cluster: 0, system_cluster: 0 }]);
    }

    #[test]
    fn resolve_session_echo_of_an_unlabeled_system_cluster_stays_tagged_echo() {
        // Orthogonal to the only stored speaker and too little speech to
        // enroll: the system cluster itself resolves to Unlabeled.
        let stored_speakers = vec![stored(10, "Alice", vec![unit(4, 1)])];
        let sys_emb = unit(4, 0); // sim to Alice = 0.0
        let mic_emb = normalize(vec![1.0, 0.05, 0.0, 0.0]); // echo of the system cluster
        let system_clusters = vec![centroid(0, sys_emb, SHORT)];
        let mic_clusters = vec![centroid(0, mic_emb, LONG)];
        let resolution = resolve_session(&system_clusters, &mic_clusters, &stored_speakers);

        assert_eq!(resolution.system[0].outcome, MatchOutcome::Unlabeled);
        assert_eq!(resolution.mic, vec![MicResolution::Echo { cluster: 0, system_cluster: 0 }]);
    }

    #[test]
    fn resolve_session_echo_of_a_newly_enrolled_system_cluster_stays_tagged_echo() {
        let sys_emb = unit(4, 0);
        let mic_emb = normalize(vec![1.0, 0.05, 0.0, 0.0]); // echo of the system cluster
        let system_clusters = vec![centroid(0, sys_emb, LONG)]; // no stored speakers: enrolls new
        let mic_clusters = vec![centroid(0, mic_emb, LONG)];
        let resolution = resolve_session(&system_clusters, &mic_clusters, &[]);

        assert_eq!(
            resolution.system[0].outcome,
            MatchOutcome::NewSpeaker { name: "Speaker 1".to_string() }
        );
        assert_eq!(resolution.mic, vec![MicResolution::Echo { cluster: 0, system_cluster: 0 }]);
    }

    #[test]
    fn resolve_session_mic_cluster_below_echo_threshold_matches_independently() {
        // Orthogonal to the system cluster: not echo, resolved on its own.
        let system_clusters = vec![centroid(0, unit(4, 0), LONG)];
        let mic_clusters = vec![centroid(0, unit(4, 1), LONG)];
        let resolution = resolve_session(&system_clusters, &mic_clusters, &[]);

        assert_eq!(
            resolution.system[0].outcome,
            MatchOutcome::NewSpeaker { name: "Speaker 1".to_string() }
        );
        match &resolution.mic[..] {
            [MicResolution::Own(decision)] => {
                assert_eq!(decision.cluster, 0);
                assert_eq!(decision.outcome, MatchOutcome::NewSpeaker { name: "Speaker 2".to_string() });
            }
            other => panic!("expected a single Own resolution, got {other:?}"),
        }
    }

    #[test]
    fn resolve_session_coordinates_claims_across_lanes() {
        // Both clusters are well clear of the 0.7 echo threshold against each
        // other, but each independently clears MATCH_THRESHOLD against the
        // single stored speaker, with the system cluster the stronger match.
        // Combined matching must let only one of them win the claim.
        let alice = unit(5, 0);
        let system_emb = normalize(vec![1.0, 0.5, 0.0, 0.0, 0.0]); // sim to alice ~0.894
        let mic_emb = normalize(vec![1.0, -0.5, 0.9, 0.0, 0.0]); // sim to alice ~0.697
        assert!(
            cosine_similarity(&system_emb, &mic_emb) < CROSS_LANE_ECHO_THRESHOLD,
            "test fixture must not be flagged as echo"
        );
        let stored_speakers = vec![stored(10, "Alice", vec![alice])];
        let system_clusters = vec![centroid(0, system_emb, LONG)];
        let mic_clusters = vec![centroid(0, mic_emb, LONG)];
        let resolution = resolve_session(&system_clusters, &mic_clusters, &stored_speakers);

        assert_eq!(resolution.system[0].outcome, MatchOutcome::Matched { speaker_id: 10 });
        match &resolution.mic[..] {
            [MicResolution::Own(decision)] => {
                assert_eq!(
                    decision.outcome,
                    MatchOutcome::Unlabeled,
                    "the weaker cross-lane claim on the same stored speaker must lose, not double-map"
                );
            }
            other => panic!("expected a single Own resolution, got {other:?}"),
        }
    }

    #[test]
    fn resolve_session_identical_cluster_ids_across_lanes_do_not_collide() {
        // Both lanes number their first cluster "0"; they must not be
        // conflated by the internal retagging used to combine them.
        let system_clusters = vec![centroid(0, unit(4, 0), LONG)];
        let mic_clusters = vec![centroid(0, unit(4, 1), LONG)]; // orthogonal: not echo
        let resolution = resolve_session(&system_clusters, &mic_clusters, &[]);

        assert_eq!(resolution.system.len(), 1);
        assert_eq!(resolution.system[0].cluster, 0);
        match &resolution.mic[..] {
            [MicResolution::Own(decision)] => assert_eq!(decision.cluster, 0),
            other => panic!("expected a single Own resolution, got {other:?}"),
        }
        // Distinct clusters, so distinct enrolled names, not a single shared one.
        let names: Vec<String> = std::iter::once(&resolution.system[0])
            .chain(resolution.mic.iter().map(|m| match m {
                MicResolution::Own(d) => d,
                MicResolution::Echo { .. } => panic!("unexpected echo"),
            }))
            .map(|d| match &d.outcome {
                MatchOutcome::NewSpeaker { name } => name.clone(),
                other => panic!("expected NewSpeaker, got {other:?}"),
            })
            .collect();
        assert_eq!(names, vec!["Speaker 1".to_string(), "Speaker 2".to_string()]);
    }

    #[test]
    fn resolve_session_mic_only_matches_exactly_like_match_speakers_alone() {
        let stored_speakers = vec![stored(10, "Alice", vec![unit(4, 0)])];
        let mic_clusters = vec![centroid(0, unit(4, 0), LONG), centroid(1, unit(4, 1), LONG)];
        let resolution = resolve_session(&[], &mic_clusters, &stored_speakers);

        assert!(resolution.system.is_empty());
        let direct = match_speakers(&mic_clusters, &stored_speakers);
        let via_resolve: Vec<MatchDecision> = resolution
            .mic
            .into_iter()
            .map(|m| match m {
                MicResolution::Own(d) => d,
                MicResolution::Echo { .. } => panic!("empty system lane can never produce echo"),
            })
            .collect();
        assert_eq!(via_resolve, direct);
    }

    #[test]
    fn resolve_session_system_only_matches_exactly_like_match_speakers_alone() {
        let stored_speakers = vec![stored(10, "Alice", vec![unit(4, 0)])];
        let system_clusters = vec![centroid(0, unit(4, 0), LONG), centroid(1, unit(4, 1), LONG)];
        let resolution = resolve_session(&system_clusters, &[], &stored_speakers);

        assert!(resolution.mic.is_empty());
        assert_eq!(resolution.system, match_speakers(&system_clusters, &stored_speakers));
    }

    #[test]
    fn resolve_session_both_lanes_empty_returns_empty_resolution() {
        let stored_speakers = vec![stored(10, "Alice", vec![unit(4, 0)])];
        let resolution = resolve_session(&[], &[], &stored_speakers);
        assert!(resolution.system.is_empty());
        assert!(resolution.mic.is_empty());
    }
}
