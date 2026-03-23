import type { TranscriptionSegment } from "../types";
import { formatTimestamp } from "./format";

export type Paragraph = { timestamp: string; text: string };

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
