import type { TranscriptionSegment } from "../../types";

/** Write `newText` onto the listed emission indices, in display order.
 * Indices that are out of range are ignored. Other segments are left alone. */
export function redistributeSegmentTexts(
  segments: TranscriptionSegment[],
  indices: number[],
  newText: string,
): void {
  const words = newText.trim().split(/\s+/).filter(Boolean);
  const valid = indices.filter((index) => index >= 0 && index < segments.length);
  if (valid.length === 0) return;
  if (valid.length === 1) {
    segments[valid[0]].text = newText.trim();
    return;
  }
  for (let i = 0; i < valid.length; i++) {
    if (i + 1 < valid.length) {
      segments[valid[i]].text = words[i] ?? "";
    } else {
      segments[valid[i]].text = words.slice(i).join(" ");
    }
  }
}
