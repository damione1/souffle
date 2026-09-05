/**
 * Live elapsed time for an in-progress recording.
 *
 * Works off a wall-clock anchor rather than a tick count. A `setInterval`
 * counter silently loses minutes because WebKit throttles timers while the
 * window is backgrounded or occluded, which is exactly where Souffle sits
 * for the whole of a meeting.
 */

/** Whole seconds between the anchor and `nowMs`. Never negative; 0 with no anchor. */
export function elapsedSecondsSince(anchorMs: number | null, nowMs: number): number {
  if (anchorMs === null) return 0;
  return Math.max(0, Math.floor((nowMs - anchorMs) / 1000));
}
