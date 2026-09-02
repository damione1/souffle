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

/** Keep paragraphs whose last word is newer than this many seconds before
 * the latest segment. Covers dual-lane ASR delay so a late Me word can
 * still land between Them paragraphs instead of freezing at the tail. */
const TAIL_WINDOW_S = 8;
/** Safety cap so a burst of tiny fragments cannot keep the tail unbounded. */
const MAX_TAIL_PARAGRAPHS = 16;

function segEnd(seg: TranscriptionSegment): number {
  return seg.end_time || seg.start_time;
}

function paragraphEndTime(
  ordered: TranscriptionSegment[],
  range: ParagraphRange,
): number {
  let end = 0;
  for (let i = range.start; i < range.end; i++) {
    end = Math.max(end, segEnd(ordered[i]));
  }
  return end;
}

/**
 * Incremental paragraph grouper for a live transcript stream.
 *
 * Batch grouping (`groupIntoParagraphs`) re-scans every segment on every
 * call, which is O(n) per call and O(n^2) over a whole meeting. This grouper
 * instead freezes paragraphs once they sit outside a trailing time window
 * (and only re-groups the small "tail" of still-open paragraphs on each
 * `append`).
 *
 * Correctness: for an in-order stream this makes `[...committed, ...tail]`
 * byte-identical to `groupIntoParagraphs(allSegments, pauseThreshold)`.
 *
 * Diarization: mic and system audio are transcribed on independent lanes.
 * Segments are kept in emission order (the batch grouper owns lane merge);
 * the time window, not a 2-paragraph cap, is what keeps a late lane from
 * landing after already-frozen speech.
 */
export function createLiveTranscript(pauseThreshold: number) {
  let committed = $state<LiveParagraph[]>([]);
  let tail = $state<LiveParagraph[]>([]);
  let tentative = $state("");
  let segmentCount = $state(0);

  // Not reactive state on purpose: only the derived paragraphs need to drive
  // rendering, and re-sorting/regrouping this buffer never touches anything
  // outside the (bounded) tail.
  let tailSegments: IndexedSegment[] = [];
  let nextParagraphId = 0;

  function globalRange(
    range: ParagraphRange,
    ordered: TranscriptionSegment[],
  ): ParagraphRange {
    const indices: number[] = [];
    for (let i = range.start; i < range.end; i++) {
      const found = tailSegments.find((item) => item.segment === ordered[i]);
      if (found) indices.push(found.index);
    }
    if (indices.length === 0) {
      return range;
    }
    return { start: Math.min(...indices), end: Math.max(...indices) + 1 };
  }

  function assignIds(paragraphs: Paragraph[], prev: LiveParagraph[]): number[] {
    const unused = [...prev];
    return paragraphs.map((paragraph) => {
      const matchAt = unused.findIndex(
        (old) =>
          old.speaker === paragraph.speaker
          && Math.abs(old.startTime - paragraph.startTime) < 0.05,
      );
      if (matchAt >= 0) {
        const [match] = unused.splice(matchAt, 1);
        return match.id;
      }
      return nextParagraphId++;
    });
  }

  function regroupTail() {
    const input = tailSegments.map((entry) => entry.segment);
    const { paragraphs, ranges, ordered } = groupIntoParagraphsWithRanges(input, pauseThreshold);
    const prevTail = tail;

    const latestStart = input.reduce((max, seg) => Math.max(max, seg.start_time), 0);
    const horizon = latestStart - TAIL_WINDOW_S;
    let numToCommit = 0;
    for (let i = 0; i < paragraphs.length; i++) {
      if (paragraphEndTime(ordered, ranges[i]) < horizon) numToCommit = i + 1;
      else break;
    }
    numToCommit = Math.max(numToCommit, paragraphs.length - MAX_TAIL_PARAGRAPHS);
    if (numToCommit === paragraphs.length && paragraphs.length > 0) {
      numToCommit -= 1;
    }

    const ids = assignIds(paragraphs, prevTail);

    for (let i = 0; i < numToCommit; i++) {
      committed.push({
        ...paragraphs[i],
        id: ids[i],
        segmentRange: globalRange(ranges[i], ordered),
      });
    }
    if (numToCommit > 0) {
      const consumed = new Set(ordered.slice(0, ranges[numToCommit - 1].end));
      tailSegments = tailSegments.filter((entry) => !consumed.has(entry.segment));
    }

    const remaining = paragraphs.slice(numToCommit);
    const remainingRanges = ranges.slice(numToCommit);
    tail = remaining.map((paragraph, i) => ({
      ...paragraph,
      id: ids[numToCommit + i],
      segmentRange: globalRange(remainingRanges[i], ordered),
    }));
  }

  function append(segment: TranscriptionSegment, segmentIndex: number) {
    if (!segment.is_final) {
      tentative = segment.text;
      return;
    }
    tentative = "";
    segmentCount++;

    tailSegments.push({ segment, index: segmentIndex });
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
