import type { MeetingRecordingSession, TranscriptionSegment } from "../types";
import { formatTimestamp } from "./format";

export type Paragraph = { timestamp: string; text: string };
export type TranscriptParagraphBlock = Paragraph & { type: "paragraph" };
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

/**
 * Group segments into flowing paragraphs with a leading timestamp.
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
export function groupIntoParagraphs(
  segments: TranscriptionSegment[],
  pauseThreshold: number,
): Paragraph[] {
  if (segments.length === 0) return [];

  const paragraphs: Paragraph[] = [];
  let currentTimestamp = formatTimestamp(segments[0].start_time);
  let currentWords: string[] = [];
  let currentChars = 0;
  let sentenceCount = 0;
  let endsSentence = false;
  let lastEnd = segments[0].start_time;

  const flush = (nextStart: number) => {
    paragraphs.push({ timestamp: currentTimestamp, text: currentWords.join(" ") });
    currentTimestamp = formatTimestamp(nextStart);
    currentWords = [];
    currentChars = 0;
    sentenceCount = 0;
    endsSentence = false;
  };

  for (const seg of segments) {
    const text = seg.text.trim();
    if (!text) continue;

    if (currentWords.length > 0) {
      const gap = seg.start_time - lastEnd;
      const breakAtSentence =
        endsSentence
        && (gap >= pauseThreshold
          || sentenceCount >= MAX_SENTENCES_PER_PARAGRAPH
          || currentChars >= SOFT_MAX_CHARS);
      const breakHard = currentChars >= HARD_MAX_CHARS;

      if (breakAtSentence || breakHard) {
        flush(seg.start_time);
      }
    }

    currentWords.push(text);
    currentChars += text.length + 1;
    sentenceCount += countSentenceEnds(text);
    endsSentence = SENTENCE_END.test(text);
    lastEnd = Math.max(lastEnd, seg.end_time || seg.start_time);
  }

  if (currentWords.length > 0) {
    paragraphs.push({ timestamp: currentTimestamp, text: currentWords.join(" ") });
  }

  return paragraphs;
}

function toParagraphBlocks(paragraphs: Paragraph[]): TranscriptParagraphBlock[] {
  return paragraphs.map((paragraph) => ({
    type: "paragraph",
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

  const normalizedSavedSessions = [...recordingSessions]
    .map((session) => ({
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
    return toParagraphBlocks(groupIntoParagraphs(segments, pauseThreshold));
  }

  const blocks: TranscriptBlock[] = [];

  const appendSession = (
    sessionSegments: TranscriptionSegment[],
    isFirstSession: boolean,
    isLiveSession: boolean,
  ) => {
    if (sessionSegments.length === 0) return;

    if (!isFirstSession) {
      blocks.push({
        type: "session-break",
        endLabel: "End of previous recording",
        startLabel: isLiveSession ? "Resumed recording in progress" : "New recording session started",
      });
    }

    blocks.push(...toParagraphBlocks(groupIntoParagraphs(sessionSegments, pauseThreshold)));
  };

  let appendedAnySession = false;
  let consumedUntil = 0;

  for (const session of normalizedSavedSessions) {
    const start = Math.max(consumedUntil, session.start);
    if (start > consumedUntil) {
      appendSession(segments.slice(consumedUntil, start), !appendedAnySession, false);
      appendedAnySession = true;
    }

    appendSession(segments.slice(start, session.end), !appendedAnySession, false);
    appendedAnySession = appendedAnySession || session.end > start;
    consumedUntil = Math.max(consumedUntil, session.end);
  }

  if (hasLiveSession && liveSessionStartIndex !== null) {
    const start = Math.max(consumedUntil, liveSessionStartIndex);
    appendSession(segments.slice(start), !appendedAnySession, true);
    return blocks;
  }

  if (consumedUntil < segments.length) {
    appendSession(segments.slice(consumedUntil), !appendedAnySession, false);
  }

  return blocks;
}
