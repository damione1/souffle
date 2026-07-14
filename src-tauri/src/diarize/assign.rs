//! Maps a `diarize()` result's time ranges onto a recording session's
//! transcript segments by maximum temporal overlap. Pure decision logic
//! only, mirroring `persist.rs`: the caller (`pipeline::diarize_task`) turns
//! the result into DB writes.

use super::DiarizedSegment;

/// one transcript segment's time span, local to a single recording session
/// (matching the per-session zero-based clock `TranscriptionSegment.start_time`
/// / `end_time` already use). `has_speaker` marks a segment whose label must
/// not be overwritten for this pass: mic pass locks Them and persistent
/// speakers; system pass locks Me and persistent speakers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentSpan {
    pub start_s: f64,
    pub end_s: f64,
    pub has_speaker: bool,
}

/// For each entry in `segments`, the local cluster index (`DiarizedSegment.speaker`)
/// with the greatest temporal overlap, or `None` if the segment already has a
/// speaker, has zero/negative duration, or no diarized range overlaps it at
/// all. Output is index-aligned with `segments`.
pub fn assign_by_overlap(diarized: &[DiarizedSegment], segments: &[SegmentSpan]) -> Vec<Option<usize>> {
    segments
        .iter()
        .map(|segment| {
            if segment.has_speaker || segment.end_s <= segment.start_s {
                return None;
            }
            best_overlap(diarized, segment)
        })
        .collect()
}

fn best_overlap(diarized: &[DiarizedSegment], segment: &SegmentSpan) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for d in diarized {
        let d_start = d.start_ms as f64 / 1000.0;
        let d_end = d.end_ms as f64 / 1000.0;
        let overlap = (segment.end_s.min(d_end) - segment.start_s.max(d_start)).max(0.0);
        if overlap <= 0.0 {
            continue;
        }
        match best {
            Some((_, best_overlap)) if best_overlap >= overlap => {}
            _ => best = Some((d.speaker, overlap)),
        }
    }
    best.map(|(speaker, _)| speaker)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start_ms: u64, end_ms: u64, speaker: usize) -> DiarizedSegment {
        DiarizedSegment { start_ms, end_ms, speaker }
    }

    fn span(start_s: f64, end_s: f64) -> SegmentSpan {
        SegmentSpan { start_s, end_s, has_speaker: false }
    }

    #[test]
    fn empty_diarized_result_leaves_everything_unassigned() {
        let segments = vec![span(0.0, 1.0), span(1.0, 2.0)];
        assert_eq!(assign_by_overlap(&[], &segments), vec![None, None]);
    }

    #[test]
    fn empty_segments_returns_empty() {
        let diarized = vec![seg(0, 1000, 0)];
        assert!(assign_by_overlap(&diarized, &[]).is_empty());
    }

    #[test]
    fn exact_overlap_matches_the_covering_cluster() {
        let diarized = vec![seg(0, 1000, 0), seg(1000, 2000, 1)];
        let segments = vec![span(0.0, 1.0), span(1.0, 2.0)];
        assert_eq!(assign_by_overlap(&diarized, &segments), vec![Some(0), Some(1)]);
    }

    #[test]
    fn single_cluster_covering_everything_assigns_all_segments_to_it() {
        let diarized = vec![seg(0, 10_000, 0)];
        let segments = vec![span(0.0, 1.0), span(2.0, 3.0), span(9.0, 9.5)];
        assert_eq!(assign_by_overlap(&diarized, &segments), vec![Some(0), Some(0), Some(0)]);
    }

    #[test]
    fn segment_outside_every_diarized_range_is_unassigned() {
        let diarized = vec![seg(0, 1000, 0)];
        let segments = vec![span(5.0, 6.0)];
        assert_eq!(assign_by_overlap(&diarized, &segments), vec![None]);
    }

    #[test]
    fn picks_the_cluster_with_greater_overlap_duration() {
        // Segment [0.9, 2.1]s overlaps cluster 0 by 0.1s and cluster 1 by 1.0s.
        let diarized = vec![seg(0, 1000, 0), seg(1000, 3000, 1)];
        let segments = vec![span(0.9, 2.1)];
        assert_eq!(assign_by_overlap(&diarized, &segments), vec![Some(1)]);
    }

    #[test]
    fn already_labeled_segment_is_never_reassigned_even_with_full_overlap() {
        let diarized = vec![seg(0, 1000, 0)];
        let segments = vec![SegmentSpan { start_s: 0.0, end_s: 1.0, has_speaker: true }];
        assert_eq!(assign_by_overlap(&diarized, &segments), vec![None]);
    }

    #[test]
    fn zero_or_negative_duration_segment_is_unassigned() {
        let diarized = vec![seg(0, 1000, 0)];
        let segments = vec![span(0.5, 0.5), span(0.8, 0.3)];
        assert_eq!(assign_by_overlap(&diarized, &segments), vec![None, None]);
    }

    #[test]
    fn overlapping_diarized_ranges_pick_the_larger_contributor() {
        // Two diarized ranges both cover the segment; cluster 1's range
        // contributes more overlap.
        let diarized = vec![seg(0, 1500, 0), seg(500, 3000, 1)];
        let segments = vec![span(0.0, 3.0)];
        // cluster 0 overlap: [0,1.5] -> 1.5s; cluster 1 overlap: [0.5,3.0] -> 2.5s
        assert_eq!(assign_by_overlap(&diarized, &segments), vec![Some(1)]);
    }

    #[test]
    fn adjacent_non_overlapping_ranges_assign_by_boundary() {
        let diarized = vec![seg(0, 1000, 0), seg(1000, 2000, 1)];
        // Segment sits exactly on the boundary; overlap with cluster 0 is 0,
        // overlap with cluster 1 is the full 0.5s.
        let segments = vec![span(1.0, 1.5)];
        assert_eq!(assign_by_overlap(&diarized, &segments), vec![Some(1)]);
    }
}
