import type { Paragraph } from "../../utils/paragraphs";
import { groupIntoParagraphsWithRanges } from "../../utils/paragraphs";
import type { TranscriptionSegment } from "../../types";

/** A grouped paragraph with a stable id for keyed rendering. Ids are assigned
 * once, when a paragraph first comes into existence (either at commit time,
 * or the first time it appears in the tail), and never reassigned while the
 * paragraph keeps growing. */
export type LiveParagraph = Paragraph & { id: number };

/**
 * Incremental paragraph grouper for a live transcript stream.
 *
 * Batch grouping (`groupIntoParagraphs`) re-scans every segment on every
 * call, which is O(n) per call and O(n^2) over a whole meeting. This grouper
 * instead freezes paragraphs as soon as a later segment proves they are
 * closed (a new paragraph started after them) and only re-groups the small
 * "tail" of still-open paragraphs on each `append`.
 *
 * Correctness: paragraph state (sentence count, char count, pause gap) only
 * ever depends on segments at or after the paragraph's own start, so once a
 * later paragraph has started, earlier paragraphs can never change. For an
 * in-order stream this makes `[...committed, ...tail]` byte-identical to
 * `groupIntoParagraphs(allSegments, pauseThreshold)`.
 *
 * The one caveat is diarization: mic and system audio are transcribed on
 * independent lanes and merged by timestamp, so a final segment can arrive
 * slightly out of order relative to a lane that's already been committed. In
 * that rare case the late segment is simply inserted at the tail (bounded
 * staleness) rather than reopening an already-frozen paragraph. This is
 * acceptable for the live view; the post-stop transcript regroups everything
 * from the database, so nothing is lost.
 */
export function createLiveTranscript(pauseThreshold: number) {
  /** At most this many trailing paragraphs are kept open (not yet frozen). */
  const MAX_TAIL_PARAGRAPHS = 2;

  let committed = $state<LiveParagraph[]>([]);
  let tail = $state<LiveParagraph[]>([]);
  let tentative = $state("");
  let segmentCount = $state(0);

  // Not reactive state on purpose: only the derived paragraphs need to drive
  // rendering, and re-sorting/regrouping this buffer never touches anything
  // outside the (bounded) tail.
  let tailSegments: TranscriptionSegment[] = [];
  let nextParagraphId = 0;

  function insertByStartTime(seg: TranscriptionSegment) {
    // Fast path: in-order arrival, the overwhelming common case.
    if (tailSegments.length === 0 || seg.start_time >= tailSegments[tailSegments.length - 1].start_time) {
      tailSegments.push(seg);
      return;
    }
    let lo = 0;
    let hi = tailSegments.length;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if (tailSegments[mid].start_time <= seg.start_time) lo = mid + 1;
      else hi = mid;
    }
    tailSegments.splice(lo, 0, seg);
  }

  function regroupTail() {
    const { paragraphs, ranges, ordered } = groupIntoParagraphsWithRanges(tailSegments, pauseThreshold);
    const prevTail = tail;
    const numToCommit = Math.max(0, paragraphs.length - MAX_TAIL_PARAGRAPHS);

    // A committed paragraph is always a prefix of the previous tail (new
    // paragraphs only ever appear at the end), so it keeps the id it was
    // already assigned while still open rather than getting a new one now.
    let cutIndex = 0;
    for (let i = 0; i < numToCommit; i++) {
      const id = i < prevTail.length ? prevTail[i].id : nextParagraphId++;
      committed.push({ ...paragraphs[i], id });
      cutIndex = ranges[i].end;
    }
    if (numToCommit > 0) tailSegments = ordered.slice(cutIndex);

    // Reassign ids: a paragraph that already existed in the previous tail
    // keeps its id (so keyed rendering treats it as the same, growing node);
    // only a genuinely new trailing paragraph gets a fresh id.
    const remaining = paragraphs.slice(numToCommit);
    const survivingOldCount = Math.max(0, prevTail.length - numToCommit);
    tail = remaining.map((paragraph, i) =>
      i < survivingOldCount
        ? { ...paragraph, id: prevTail[numToCommit + i].id }
        : { ...paragraph, id: nextParagraphId++ },
    );
  }

  function append(segment: TranscriptionSegment) {
    if (!segment.is_final) {
      tentative = segment.text;
      return;
    }
    tentative = "";
    segmentCount++;

    const diarizedSoFar = segment.speaker != null || tailSegments.some((s) => s.speaker != null);
    if (diarizedSoFar) insertByStartTime(segment);
    else tailSegments.push(segment);

    regroupTail();
  }

  function reset() {
    tailSegments = [];
    committed = [];
    tail = [];
    tentative = "";
    segmentCount = 0;
    nextParagraphId = 0;
  }

  return {
    append,
    reset,
    get committed() { return committed; },
    get tail() { return tail; },
    get tentative() { return tentative; },
    get segmentCount() { return segmentCount; },
  };
}
