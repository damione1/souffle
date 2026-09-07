/** HTMLMediaElement.HAVE_METADATA — `currentTime` can be set. */
export const HAVE_METADATA = 1;

export type PendingSeek = {
  seekSeconds: number;
  /** Media src this seek was armed for. A stale loadedmetadata from another
   * file must not apply this offset. */
  src: string;
};

export type SeekableMedia = {
  readyState: number;
  currentTime: number;
  src?: string;
  currentSrc?: string;
  play: () => Promise<void> | void;
};

function mediaSrc(el: SeekableMedia): string {
  return el.src || el.currentSrc || "";
}

/** Wait for loadedmetadata when the file is swapping or metadata is not here yet. */
export function seekNeedsMetadata(readyState: number, sessionChanged: boolean): boolean {
  return sessionChanged || readyState < HAVE_METADATA;
}

/** Apply a pending seek if metadata is available on the file it was armed for.
 * Returns remaining pending seek. */
export function applyPendingSeek(
  el: SeekableMedia | undefined | null,
  pending: PendingSeek | null,
): PendingSeek | null {
  if (!el || !pending) return pending;
  if (el.readyState < HAVE_METADATA) return pending;
  const actual = mediaSrc(el);
  if (actual && actual !== pending.src) return pending;
  el.currentTime = pending.seekSeconds;
  void el.play();
  return null;
}
