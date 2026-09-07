/** HTMLMediaElement.HAVE_METADATA — `currentTime` can be set. */
export const HAVE_METADATA = 1;

export type PendingSeek = {
  seekSeconds: number;
};

export type SeekableMedia = {
  readyState: number;
  currentTime: number;
  play: () => Promise<void> | void;
};

/** Wait for loadedmetadata when the file is swapping or metadata is not here yet. */
export function seekNeedsMetadata(readyState: number, sessionChanged: boolean): boolean {
  return sessionChanged || readyState < HAVE_METADATA;
}

/** Apply a pending seek if metadata is available. Returns remaining pending seek. */
export function applyPendingSeek(
  el: SeekableMedia | undefined | null,
  pending: PendingSeek | null,
): PendingSeek | null {
  if (!el || !pending) return pending;
  if (el.readyState < HAVE_METADATA) return pending;
  el.currentTime = pending.seekSeconds;
  void el.play();
  return null;
}
