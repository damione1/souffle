import type { MeetingRecordingSession, Speaker, TranscriptionSegment } from "../types";
import { formatTimestamp } from "./format";

export type Paragraph = { timestamp: string; startTime: number; text: string; speaker?: Speaker | null };
export type TranscriptParagraphBlock = Paragraph & {
  type: "paragraph";
  /** Position of this paragraph's recording session in `recordingSessions`
   * (see `buildMeetingTranscriptBlocks`), or `null` when it can't be
   * attributed to a known session (legacy data, or a gap not covered by any
   * saved session). Used to map a paragraph to its playable audio file —
   * see `features/meeting/audio-map.ts`. */
  recordingSessionIndex: number | null;
  /** Half-open `[start, end)` indices into the full `segments` array
   * (equal to segment `sort_order` values). Used when retagging a turn. */
  segmentRange: ParagraphRange;
};
export type TranscriptSessionBreakBlock = {
  type: "session-break";
  endLabel: string;
  startLabel: string;
};
export type TranscriptBlock = TranscriptParagraphBlock | TranscriptSessionBreakBlock;

/** Paragraph break after this many sentences, even with no pause. */
const MAX_SENTENCES_PER_PARAGRAPH = 4;
/** Once a paragraph reaches this length, break at the next sentence end. */
const SOFT_MAX_CHARS = 480;
/** Absolute ceiling for streams with no punctuation at all. */
const HARD_MAX_CHARS = 700;

/** Gap after which a new speaker is treated as a handoff, not crosstalk. */
const HANDOFF_GAP_S = 0.35;
/** Overlapping speech closes the interrupted turn this long after the
 * interrupter started, even without a sentence end. Only applied when the
 * interrupted turn is long enough to be a monologue — short simultaneous
 * phrases stay intact. */
const INTERRUPT_HOLD_S = 1.0;
/** Turns shorter than this are treated as phrases, not monologues, so a
 * 1s hold must not peel the last word off word-level crosstalk. */
const MONOLOGUE_MIN_S = 2.0;

/** Sentence-ending punctuation, allowing closing quotes/brackets after it. */
const SENTENCE_END = /[.!?…]["»”')\]]*\s*$/;

function countSentenceEnds(text: string): number {
  return (text.match(/[.!?…]+(?=["»”')\]]*(\s|$))/g) ?? []).length;
}

function segEnd(seg: TranscriptionSegment): number {
  return seg.end_time || seg.start_time;
}

/** Half-open index range `[start, end)` into the ordered segment array that a
 * paragraph was built from. */
export type ParagraphRange = { start: number; end: number };

type Turn = {
  start: number;
  lastEnd: number;
  speaker: Speaker | null;
  segments: TranscriptionSegment[];
};

/**
 * Cluster diarized segments into per-speaker turns.
 *
 * Mic (Me) and system audio (Them) are transcribed on independent lanes.
 * Within a lane, emission order is kept (timestamps can jitter or overlap
 * after a KV refresh; time-sorting the same speaker zippers two hypotheses
 * into word salad). Lanes are pause-split, then overlapping turns from
 * another speaker split the interrupted turn so the interruption can sort
 * between the two halves:
 * - Sequential handoff (>= 350ms after the other speaker's last end): close
 *   immediately.
 * - Overlap / tight interjection: close at the earlier of the next sentence
 *   end or `INTERRUPT_HOLD_S` after the interrupter started.
 */
function clusterIntoTurns(segments: TranscriptionSegment[], pauseThreshold: number): Turn[] {
  const lanes = new Map<Speaker, TranscriptionSegment[]>();
  const untagged: TranscriptionSegment[] = [];

  for (const seg of segments) {
    const speaker = seg.speaker;
    if (speaker == null) {
      untagged.push(seg);
      continue;
    }
    const lane = lanes.get(speaker);
    if (lane) lane.push(seg);
    else lanes.set(speaker, [seg]);
  }

  const turns: Turn[] = [];
  for (const [speaker, segs] of lanes) {
    turns.push(...pauseSplitLane(segs, speaker, pauseThreshold));
  }
  for (const seg of untagged) {
    turns.push({
      start: seg.start_time,
      lastEnd: segEnd(seg),
      speaker: null,
      segments: [seg],
    });
  }

  return splitInterruptedTurns(turns);
}

function pauseSplitLane(
  segs: TranscriptionSegment[],
  speaker: Speaker,
  pauseThreshold: number,
): Turn[] {
  const turns: Turn[] = [];
  let current: Turn | null = null;
  for (const seg of segs) {
    if (current && seg.start_time - current.lastEnd < pauseThreshold) {
      current.segments.push(seg);
      current.lastEnd = Math.max(current.lastEnd, segEnd(seg));
    } else {
      current = {
        start: seg.start_time,
        lastEnd: segEnd(seg),
        speaker,
        segments: [seg],
      };
      turns.push(current);
    }
  }
  return turns;
}

/** Index at which `turn` should split so `interrupter` can sort between the
 * two halves. `null` when the overlap is too short / has no split point. */
function interruptSplitIndex(turn: Turn, interrupter: Turn): number | null {
  const spokenBefore = turn.segments.findIndex((s) => s.start_time >= interrupter.start);
  const headLen = spokenBefore === -1 ? turn.segments.length : spokenBefore;
  const lastBeforeEnd = headLen > 0 ? segEnd(turn.segments[headLen - 1]) : turn.start;

  if (interrupter.start >= lastBeforeEnd + HANDOFF_GAP_S) {
    return headLen > 0 && headLen < turn.segments.length ? headLen : null;
  }

  const holdAt = interrupter.start + INTERRUPT_HOLD_S;
  const applyHold = turn.lastEnd - turn.start >= MONOLOGUE_MIN_S;
  for (let i = headLen; i < turn.segments.length; i++) {
    const seg = turn.segments[i];
    if (SENTENCE_END.test(seg.text.trim())) return i + 1;
    if (applyHold && seg.start_time >= holdAt) return i;
  }
  return null;
}

function splitInterruptedTurns(turns: Turn[]): Turn[] {
  const remaining = [...turns];
  const out: Turn[] = [];

  while (remaining.length > 0) {
    remaining.sort((a, b) => a.start - b.start);
    const turn = remaining.shift()!;
    if (turn.speaker == null || turn.segments.length === 0) {
      out.push(turn);
      continue;
    }

    const interrupter = remaining.find(
      (other) =>
        other.speaker != null
        && other.speaker !== turn.speaker
        && other.start < turn.lastEnd
        && other.start >= turn.start,
    );

    if (!interrupter) {
      out.push(turn);
      continue;
    }

    const splitAt = interruptSplitIndex(turn, interrupter);
    if (splitAt == null || splitAt <= 0 || splitAt >= turn.segments.length) {
      out.push(turn);
      continue;
    }

    const headSegs = turn.segments.slice(0, splitAt);
    const tailSegs = turn.segments.slice(splitAt);
    out.push({
      start: turn.start,
      lastEnd: Math.max(...headSegs.map(segEnd)),
      speaker: turn.speaker,
      segments: headSegs,
    });
    remaining.push({
      start: tailSegs[0].start_time,
      lastEnd: Math.max(...tailSegs.map(segEnd)),
      speaker: turn.speaker,
      segments: tailSegs,
    });
  }

  return out;
}

/**
 * Split a homogeneous (or non-diarized) segment stream into paragraphs.
 * Paragraphs only break at sentence boundaries, triggered by any of:
 * - a speaker change when `breakOnSpeakerChange` is set
 * - a pause ≥ `pauseThreshold` seconds before the next segment
 * - the paragraph already holds MAX_SENTENCES_PER_PARAGRAPH sentences
 * - the paragraph exceeds SOFT_MAX_CHARS
 * Streams with no punctuation at all fall back to a hard length cap.
 *
 * Negative gaps (legacy data with window-relative timestamps) never trigger
 * a pause break.
 */
function paragraphsFromSegments(
  segments: TranscriptionSegment[],
  pauseThreshold: number,
  breakOnSpeakerChange: boolean,
): { paragraphs: Paragraph[]; ranges: ParagraphRange[] } {
  if (segments.length === 0) return { paragraphs: [], ranges: [] };

  const paragraphs: Paragraph[] = [];
  const ranges: ParagraphRange[] = [];
  let rangeStart = 0;
  let currentTimestamp = formatTimestamp(segments[0].start_time);
  let currentStartTime = segments[0].start_time;
  let currentSpeaker: Speaker | null = segments[0].speaker ?? null;
  let currentWords: string[] = [];
  let currentChars = 0;
  let sentenceCount = 0;
  let endsSentence = false;
  let lastEnd = segments[0].start_time;

  const flush = (boundaryIndex: number, nextStart: number, nextSpeaker: Speaker | null) => {
    paragraphs.push({
      timestamp: currentTimestamp,
      startTime: currentStartTime,
      text: currentWords.join(" "),
      speaker: currentSpeaker,
    });
    ranges.push({ start: rangeStart, end: boundaryIndex });
    rangeStart = boundaryIndex;
    currentTimestamp = formatTimestamp(nextStart);
    currentStartTime = nextStart;
    currentSpeaker = nextSpeaker;
    currentWords = [];
    currentChars = 0;
    sentenceCount = 0;
    endsSentence = false;
  };

  for (let i = 0; i < segments.length; i++) {
    const seg = segments[i];
    const text = seg.text.trim();
    // An empty segment carries no text but still occupies an index; it stays
    // part of whichever paragraph range is currently open (or the next one,
    // if none has started yet), since ranges are tracked by index, not text.
    if (!text) continue;
    const speaker = seg.speaker ?? null;

    if (currentWords.length > 0) {
      if (breakOnSpeakerChange && speaker !== currentSpeaker) {
        flush(i, seg.start_time, speaker);
      } else {
        const gap = seg.start_time - lastEnd;
        const breakAtSentence =
          endsSentence
          && (gap >= pauseThreshold
            || sentenceCount >= MAX_SENTENCES_PER_PARAGRAPH
            || currentChars >= SOFT_MAX_CHARS);
        const breakHard = currentChars >= HARD_MAX_CHARS;

        if (breakAtSentence || breakHard) {
          flush(i, seg.start_time, speaker);
        }
      }
    } else {
      currentSpeaker = speaker;
    }

    currentWords.push(text);
    currentChars += text.length + 1;
    sentenceCount += countSentenceEnds(text);
    endsSentence = SENTENCE_END.test(text);
    lastEnd = Math.max(lastEnd, segEnd(seg));
  }

  if (currentWords.length > 0) {
    paragraphs.push({
      timestamp: currentTimestamp,
      startTime: currentStartTime,
      text: currentWords.join(" "),
      speaker: currentSpeaker,
    });
    ranges.push({ start: rangeStart, end: segments.length });
  }

  return { paragraphs, ranges };
}

/**
 * Group segments into flowing paragraphs with a leading timestamp, also
 * returning the source index range (into the returned `ordered` array) each
 * paragraph consumed. Callers that need incremental/streaming regrouping use
 * the ranges to know which segments a completed paragraph consumed, without
 * duplicating the break rules below.
 *
 * Diarized meetings: cluster into per-speaker turns (keeping emission order
 * within a lane), split each turn into readable paragraphs, then order those
 * paragraphs by start time so an interruption lands between the two halves
 * of a long turn instead of after every length-split of it.
 *
 * Non-diarized streams are left exactly as emitted (legacy window-relative
 * timestamps must not be reordered).
 */
export function groupIntoParagraphsWithRanges(
  segments: TranscriptionSegment[],
  pauseThreshold: number,
): { paragraphs: Paragraph[]; ranges: ParagraphRange[]; ordered: TranscriptionSegment[] } {
  if (segments.length === 0) return { paragraphs: [], ranges: [], ordered: segments };

  const diarized = segments.some((s) => s.speaker != null);
  if (!diarized) {
    const split = paragraphsFromSegments(segments, pauseThreshold, false);
    return { ...split, ordered: segments };
  }

  const turns = clusterIntoTurns(segments, pauseThreshold);
  const pieces: { paragraph: Paragraph; segs: TranscriptionSegment[] }[] = [];
  for (const turn of turns) {
    const split = paragraphsFromSegments(turn.segments, pauseThreshold, false);
    for (let i = 0; i < split.paragraphs.length; i++) {
      pieces.push({
        paragraph: split.paragraphs[i],
        segs: turn.segments.slice(split.ranges[i].start, split.ranges[i].end),
      });
    }
  }

  pieces.sort((a, b) => a.paragraph.startTime - b.paragraph.startTime);

  const paragraphs: Paragraph[] = [];
  const ranges: ParagraphRange[] = [];
  const ordered: TranscriptionSegment[] = [];
  for (const piece of pieces) {
    const start = ordered.length;
    ordered.push(...piece.segs);
    paragraphs.push(piece.paragraph);
    ranges.push({ start, end: ordered.length });
  }

  return { paragraphs, ranges, ordered };
}

/**
 * Group segments into flowing paragraphs with a leading timestamp. See
 * `groupIntoParagraphsWithRanges` for the break rules; this is a thin
 * wrapper that drops the ranges for callers that only need the paragraphs.
 */
export function groupIntoParagraphs(
  segments: TranscriptionSegment[],
  pauseThreshold: number,
): Paragraph[] {
  return groupIntoParagraphsWithRanges(segments, pauseThreshold).paragraphs;
}

function toParagraphBlocks(
  paragraphs: Paragraph[],
  ranges: ParagraphRange[],
  segmentOffset: number,
  recordingSessionIndex: number | null,
): TranscriptParagraphBlock[] {
  return paragraphs.map((paragraph, index) => ({
    type: "paragraph",
    recordingSessionIndex,
    segmentRange: {
      start: segmentOffset + ranges[index].start,
      end: segmentOffset + ranges[index].end,
    },
    ...paragraph,
  }));
}

/**
 * Build transcript render blocks for meeting sessions so resumed recordings are
 * visually separated from previous sessions without mutating persisted segments.
 */
export function buildMeetingTranscriptBlocks(
  segments: TranscriptionSegment[],
  recordingSessions: MeetingRecordingSession[],
  pauseThreshold: number,
  liveSessionStartIndex: number | null = null,
): TranscriptBlock[] {
  if (segments.length === 0) return [];

  // `sessionIndex` keeps each session's position in the original (already
  // chronological) `recordingSessions` array — that position is also the
  // audio filename a recorder wrote for it (see `commands::get_meeting_audio`
  // on the backend) — before sorting loses it.
  const normalizedSavedSessions = recordingSessions
    .map((session, sessionIndex) => ({
      sessionIndex,
      start: Math.max(0, Number(session.start_segment_index)),
      end: Math.min(segments.length, Number(session.end_segment_index)),
    }))
    .filter((session) => session.end > session.start)
    .sort((left, right) => left.start - right.start);

  const hasLiveSession =
    liveSessionStartIndex !== null
    && liveSessionStartIndex >= 0
    && liveSessionStartIndex < segments.length;

  if (normalizedSavedSessions.length === 0 && !hasLiveSession) {
    const grouped = groupIntoParagraphsWithRanges(segments, pauseThreshold);
    return toParagraphBlocks(grouped.paragraphs, grouped.ranges, 0, null);
  }

  const blocks: TranscriptBlock[] = [];

  const appendSession = (
    sessionSegments: TranscriptionSegment[],
    segmentOffset: number,
    isFirstSession: boolean,
    isLiveSession: boolean,
    recordingSessionIndex: number | null,
  ) => {
    if (sessionSegments.length === 0) return;

    if (!isFirstSession) {
      blocks.push({
        type: "session-break",
        endLabel: "End of previous recording",
        startLabel: isLiveSession ? "Resumed recording in progress" : "New recording session started",
      });
    }

    const grouped = groupIntoParagraphsWithRanges(sessionSegments, pauseThreshold);
    blocks.push(
      ...toParagraphBlocks(
        grouped.paragraphs,
        grouped.ranges,
        segmentOffset,
        recordingSessionIndex,
      ),
    );
  };

  let appendedAnySession = false;
  let consumedUntil = 0;

  for (const session of normalizedSavedSessions) {
    const start = Math.max(consumedUntil, session.start);
    if (start > consumedUntil) {
      // Segments not covered by any known recording session (gap in the
      // saved ranges) can't be attributed to an audio file.
      appendSession(segments.slice(consumedUntil, start), consumedUntil, !appendedAnySession, false, null);
      appendedAnySession = true;
    }

    appendSession(segments.slice(start, session.end), start, !appendedAnySession, false, session.sessionIndex);
    appendedAnySession = appendedAnySession || session.end > start;
    consumedUntil = Math.max(consumedUntil, session.end);
  }

  if (hasLiveSession && liveSessionStartIndex !== null) {
    const start = Math.max(consumedUntil, liveSessionStartIndex);
    // The in-progress session hasn't been saved yet, so it isn't in
    // `recordingSessions` — its eventual position is the next index.
    appendSession(segments.slice(start), start, !appendedAnySession, true, recordingSessions.length);
    return blocks;
  }

  if (consumedUntil < segments.length) {
    appendSession(segments.slice(consumedUntil), consumedUntil, !appendedAnySession, false, null);
  }

  return blocks;
}
