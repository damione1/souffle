import type { Paragraph, ParagraphRange } from "../../utils/paragraphs";
import { groupIntoParagraphsWithRanges } from "../../utils/paragraphs";
import type { TranscriptionSegment } from "../../types";

/** A grouped paragraph with a stable id for keyed rendering. Ids are assigned
 * once, when a paragraph first comes into existence (either at commit time,
 * or the first time it appears in the tail), and never reassigned while the
 * paragraph keeps growing. */
export type LiveParagraph = Paragraph & {
  id: number;
  /** Half-open range into the finalized segment list for this paragraph. */
  segmentRange: ParagraphRange;
};

type IndexedSegment = { segment: TranscriptionSegment; index: number };

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
  let tailSegments: IndexedSegment[] = [];
  let nextParagraphId = 0;

  function insertByStartTime(entry: IndexedSegment) {
    const seg = entry.segment;
    if (
      tailSegments.length === 0
      || seg.start_time >= tailSegments[tailSegments.length - 1].segment.start_time
    ) {
      tailSegments.push(entry);
      return;
    }
    let lo = 0;
    let hi = tailSegments.length;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if (tailSegments[mid].segment.start_time <= seg.start_time) lo = mid + 1;
      else hi = mid;
    }
    tailSegments.splice(lo, 0, entry);
  }

  function globalRange(range: ParagraphRange): ParagraphRange {
    const startIndex = tailSegments[range.start]?.index;
    const endIndex = tailSegments[range.end - 1]?.index;
    return {
      start: startIndex ?? range.start,
      end: endIndex == null ? range.end : endIndex + 1,
    };
  }

  function regroupTail() {
    const ordered = tailSegments.map((entry) => entry.segment);
    const { paragraphs, ranges } = groupIntoParagraphsWithRanges(ordered, pauseThreshold);
    const prevTail = tail;
    const numToCommit = Math.max(0, paragraphs.length - MAX_TAIL_PARAGRAPHS);

    for (let i = 0; i < numToCommit; i++) {
      const id = i < prevTail.length ? prevTail[i].id : nextParagraphId++;
      committed.push({
        ...paragraphs[i],
        id,
        segmentRange: globalRange(ranges[i]),
      });
    }
    if (numToCommit > 0) {
      const cutIndex = ranges[numToCommit - 1].end;
      tailSegments = tailSegments.slice(cutIndex);
    }

    const remaining = paragraphs.slice(numToCommit);
    const remainingRanges = ranges.slice(numToCommit);
    const survivingOldCount = Math.max(0, prevTail.length - numToCommit);
    tail = remaining.map((paragraph, i) => {
      const id = i < survivingOldCount
        ? prevTail[numToCommit + i].id
        : nextParagraphId++;
      return {
        ...paragraph,
        id,
        segmentRange: globalRange(remainingRanges[i]),
      };
    });
  }

  function append(segment: TranscriptionSegment, segmentIndex: number) {
    if (!segment.is_final) {
      tentative = segment.text;
      return;
    }
    tentative = "";
    segmentCount++;

    const entry = { segment, index: segmentIndex };
    const diarizedSoFar =
      segment.speaker != null || tailSegments.some((item) => item.segment.speaker != null);
    if (diarizedSoFar) insertByStartTime(entry);
    else tailSegments.push(entry);

    regroupTail();
  }

  /** Update a paragraph's displayed text after a live edit without reopening
   * the incremental grouper. */
  function editParagraph(id: number, newText: string): LiveParagraph | null {
    const committedIndex = committed.findIndex((paragraph) => paragraph.id === id);
    if (committedIndex !== -1) {
      committed[committedIndex] = { ...committed[committedIndex], text: newText };
      return committed[committedIndex];
    }
    const tailIndex = tail.findIndex((paragraph) => paragraph.id === id);
    if (tailIndex !== -1) {
      tail[tailIndex] = { ...tail[tailIndex], text: newText };
      return tail[tailIndex];
    }
    return null;
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
    editParagraph,
    reset,
    get committed() { return committed; },
    get tail() { return tail; },
    get tentative() { return tentative; },
    get segmentCount() { return segmentCount; },
  };
}
