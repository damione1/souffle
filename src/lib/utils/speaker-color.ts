/** Deterministic warm palette for persistent speaker badges. Indices map to
 * CSS classes in `app.css` (`.speaker-pill-N`). */
export const SPEAKER_PALETTE_SIZE = 10;

/** Stable palette index for a persistent speaker id. */
export function speakerPaletteIndex(speakerId: number): number {
  let hash = speakerId;
  hash = Math.imul(hash ^ (hash >>> 16), 0x45d9f3b);
  hash = Math.imul(hash ^ (hash >>> 13), 0x45d9f3b);
  hash ^= hash >>> 16;
  return Math.abs(hash) % SPEAKER_PALETTE_SIZE;
}

/** CSS class for a persistent speaker badge pill. */
export function speakerPillClass(speakerId: number): string {
  return `speaker-pill-${speakerPaletteIndex(speakerId)}`;
}

/** Extract a persistent speaker id from a `spk:<id>` label, or null. */
export function persistentSpeakerId(speaker: string | null | undefined): number | null {
  if (!speaker) return null;
  const match = /^spk:(\d+)$/.exec(speaker);
  return match ? Number(match[1]) : null;
}
