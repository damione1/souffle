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

/** Sentence-ending punctuation, allowing closing quotes/brackets after it. */
const SENTENCE_END = /[.!?…]["»”')\]]*\s*$/;

function countSentenceEnds(text: string): number {
  return (text.match(/[.!?…]+(?=["»”')\]]*(\s|$))/g) ?? []).length;
}

/** Half-open index range `[start, end)` into the ordered segment array that a
 * paragraph was built from. */
export type ParagraphRange = { start: number; end: number };

/**
 * Cluster time-sorted diarized segments into per-speaker turns, then emit
 * them ordered by each turn's start time (turn segments stay contiguous and
 * chronological internally). A segment joins its speaker's currently open
 * turn if the gap since that turn's last segment is under `pauseThreshold`;
 * otherwise that speaker's turn closes and a new one opens.
 *
 * Mic (Me) and system audio (Them) are transcribed on independent lanes, so
 * during crosstalk a naive time-sort interleaves them word by word. Turn
 * clustering keeps each speaker's own words together (one turn = one
 * speaker's contiguous phrase) even while both are talking over each other,
 * so the paragraph loop below breaks on turn boundaries instead of on every
 * single word.
 *
 * Without a pause, a monologue would otherwise absorb everything indefinitely,
 * so a second speaker's interjection would render far below the point in the
 * monologue it actually responded to. To keep interjections anchored near
 * their moment: opening a new turn marks every other speaker's currently open
 * turn as interrupted. An interrupted turn still keeps absorbing segments
 * (crosstalk should stay readable), but closes as soon as one of them ends a
 * sentence, so the speaker's next segment opens a fresh turn that sorts after
 * the interjection instead of the interjection trailing behind an
 * ever-growing paragraph. A turn that never hits a sentence end (no
 * punctuation) still only closes on pause, same as before.
 */
function clusterIntoTurns(sorted: TranscriptionSegment[], pauseThreshold: number): TranscriptionSegment[] {
  type Turn = { start: number; lastEnd: number; segments: TranscriptionSegment[]; interrupted: boolean };
  const openTurns = new Map<Speaker, Turn>();
  const turns: Turn[] = [];

  for (const seg of sorted) {
    const speaker = seg.speaker;
    if (speaker == null) {
      // Non-diarized segment inside an otherwise-diarized stream (shouldn't
      // normally happen): keep it as its own single-segment turn in place.
      turns.push({ start: seg.start_time, lastEnd: seg.end_time || seg.start_time, segments: [seg], interrupted: false });
      continue;
    }
    const open = openTurns.get(speaker);
    if (open && seg.start_time - open.lastEnd < pauseThreshold) {
      open.segments.push(seg);
      open.lastEnd = Math.max(open.lastEnd, seg.end_time || seg.start_time);
      if (open.interrupted && SENTENCE_END.test(seg.text.trim())) {
        // First sentence end at or after the interruption: close now so the
        // speaker's next segment starts a fresh, later-sorting turn.
        openTurns.delete(speaker);
      }
    } else {
      const turn: Turn = { start: seg.start_time, lastEnd: seg.end_time || seg.start_time, segments: [seg], interrupted: false };
      turns.push(turn);
      openTurns.set(speaker, turn);
      for (const [otherSpeaker, otherTurn] of openTurns) {
        if (otherSpeaker !== speaker) otherTurn.interrupted = true;
      }
    }
  }

  turns.sort((a, b) => a.start - b.start);
  return turns.flatMap((t) => t.segments);
}

/**
 * Group segments into flowing paragraphs with a leading timestamp, also
 * returning the source index range (into the returned `ordered` array) each
 * paragraph consumed. Callers that need incremental/streaming regrouping use
 * the ranges to know which segments a completed paragraph consumed, without
 * duplicating the break rules below.
 *
 * Paragraphs only break at sentence boundaries, triggered by any of:
 * - a pause ≥ `pauseThreshold` seconds before the next segment
 * - the paragraph already holds MAX_SENTENCES_PER_PARAGRAPH sentences
 * - the paragraph exceeds SOFT_MAX_CHARS
 * So continuous speech without pauses still yields readable paragraphs.
 * Streams with no punctuation at all fall back to a hard length cap.
 *
 * Works across engines: Kyutai emits one segment per word, Whisper and
 * Parakeet emit sentence/window segments. Negative gaps (legacy data with
 * window-relative timestamps) never trigger a pause break.
 */
export function groupIntoParagraphsWithRanges(
  segments: TranscriptionSegment[],
  pauseThreshold: number,
): { paragraphs: Paragraph[]; ranges: ParagraphRange[]; ordered: TranscriptionSegment[] } {
  if (segments.length === 0) return { paragraphs: [], ranges: [], ordered: segments };

  // Diarized meetings tag each segment with a speaker. Mic (Me) and system
  // audio (Them) are transcribed independently, so order by time first, then
  // cluster into per-speaker turns so crosstalk reads as flowing phrases
  // instead of breaking on every speaker change. Non-diarized streams are
  // left exactly as emitted (legacy window-relative timestamps must not be
  // reordered).
  const diarized = segments.some((s) => s.speaker != null);
  const ordered = diarized
    ? clusterIntoTurns([...segments].sort((a, b) => a.start_time - b.start_time), pauseThreshold)
    : segments;

  const paragraphs: Paragraph[] = [];
  const ranges: ParagraphRange[] = [];
  let rangeStart = 0;
  let currentTimestamp = formatTimestamp(ordered[0].start_time);
  let currentStartTime = ordered[0].start_time;
  let currentSpeaker: Speaker | null = ordered[0].speaker ?? null;
  let currentWords: string[] = [];
  let currentChars = 0;
  let sentenceCount = 0;
  let endsSentence = false;
  let lastEnd = ordered[0].start_time;

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

  for (let i = 0; i < ordered.length; i++) {
    const seg = ordered[i];
    const text = seg.text.trim();
    // An empty segment carries no text but still occupies an index; it stays
    // part of whichever paragraph range is currently open (or the next one,
    // if none has started yet), since ranges are tracked by index, not text.
    if (!text) continue;
    const speaker = seg.speaker ?? null;

    if (currentWords.length > 0) {
      // A speaker change always starts a new paragraph (even mid-sentence).
      if (diarized && speaker !== currentSpeaker) {
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
    lastEnd = Math.max(lastEnd, seg.end_time || seg.start_time);
  }

  if (currentWords.length > 0) {
    paragraphs.push({
      timestamp: currentTimestamp,
      startTime: currentStartTime,
      text: currentWords.join(" "),
      speaker: currentSpeaker,
    });
    ranges.push({ start: rangeStart, end: ordered.length });
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
  recordingSessionIndex: number | null,
): TranscriptParagraphBlock[] {
  return paragraphs.map((paragraph) => ({
    type: "paragraph",
    recordingSessionIndex,
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
    return toParagraphBlocks(groupIntoParagraphs(segments, pauseThreshold), null);
  }

  const blocks: TranscriptBlock[] = [];

  const appendSession = (
    sessionSegments: TranscriptionSegment[],
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

    blocks.push(...toParagraphBlocks(groupIntoParagraphs(sessionSegments, pauseThreshold), recordingSessionIndex));
  };

  let appendedAnySession = false;
  let consumedUntil = 0;

  for (const session of normalizedSavedSessions) {
    const start = Math.max(consumedUntil, session.start);
    if (start > consumedUntil) {
      // Segments not covered by any known recording session (gap in the
      // saved ranges) can't be attributed to an audio file.
      appendSession(segments.slice(consumedUntil, start), !appendedAnySession, false, null);
      appendedAnySession = true;
    }

    appendSession(segments.slice(start, session.end), !appendedAnySession, false, session.sessionIndex);
    appendedAnySession = appendedAnySession || session.end > start;
    consumedUntil = Math.max(consumedUntil, session.end);
  }

  if (hasLiveSession && liveSessionStartIndex !== null) {
    const start = Math.max(consumedUntil, liveSessionStartIndex);
    // The in-progress session hasn't been saved yet, so it isn't in
    // `recordingSessions` — its eventual position is the next index.
    appendSession(segments.slice(start), !appendedAnySession, true, recordingSessions.length);
    return blocks;
  }

  if (consumedUntil < segments.length) {
    appendSession(segments.slice(consumedUntil), !appendedAnySession, false, null);
  }

  return blocks;
}
