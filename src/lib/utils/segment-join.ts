/**
 * The separator to insert between already-transcribed text and the next
 * segment. Engines emit their own leading spaces inconsistently, so a
 * segment is joined with a single space only when neither side supplies one.
 *
 * The live preview and the finalized transcript both use this, so the word
 * shown in grey sits exactly where it lands once it is confirmed.
 */
export function segmentGap(before: string, next: string): string {
  if (!before || !next) return "";
  return before.endsWith(" ") || next.startsWith(" ") ? "" : " ";
}
