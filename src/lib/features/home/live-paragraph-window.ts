/** Only the most recent paragraphs stay in the DOM; older ones remain in
 * `committed` but are not rendered live. */
export const LIVE_PARAGRAPH_WINDOW = 30;

export function windowedParagraphs<T>(
  committed: readonly T[],
  tail: readonly T[],
  limit = LIVE_PARAGRAPH_WINDOW,
): T[] {
  return [...committed.slice(-limit), ...tail].slice(-limit);
}

/** How many leading items will unmount when the window slides forward. */
export function leadingRemovedCount<T>(previous: readonly T[], next: readonly T[]): number {
  if (previous.length === 0 || next.length === 0) return 0;
  const index = previous.indexOf(next[0]);
  return index > 0 ? index : 0;
}

export function measureLeadingHeight(
  container: Pick<HTMLElement, "children">,
  count: number,
  gap: number,
): number {
  let height = 0;
  const n = Math.min(count, container.children.length);
  for (let i = 0; i < n; i++) {
    height += (container.children[i] as HTMLElement).offsetHeight + gap;
  }
  return height;
}

export function scrollTopAfterLeadingUnmount(scrollTop: number, removedHeight: number): number {
  return Math.max(0, scrollTop - removedHeight);
}
