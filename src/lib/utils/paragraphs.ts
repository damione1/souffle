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

/** Group word-level segments into flowing paragraphs with a leading timestamp */
export function groupIntoParagraphs(
  segments: TranscriptionSegment[],
  pauseThreshold: number,
): Paragraph[] {
  if (segments.length === 0) return [];

  const paragraphs: Paragraph[] = [];
  let currentTimestamp = formatTimestamp(segments[0].start_time);
  let currentWords: string[] = [];
  let lastTime = segments[0].start_time;
  let lastText = "";

  for (const seg of segments) {
    const gap = seg.start_time - lastTime;
    const endsWithSentence = /[.!?…]\s*$/.test(lastText);

    if (currentWords.length > 0 && gap >= pauseThreshold && endsWithSentence) {
      paragraphs.push({ timestamp: currentTimestamp, text: currentWords.join(" ") });
      currentTimestamp = formatTimestamp(seg.start_time);
      currentWords = [];
    }

    currentWords.push(seg.text.trim());
    lastTime = seg.start_time;
    lastText = seg.text;
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
