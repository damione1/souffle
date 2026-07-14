/** Move a node under `document.body` so `position: fixed` escapes overflow clipping. */
export function portal(node: HTMLElement) {
  document.body.appendChild(node);
  return {
    destroy() {
      node.remove();
    },
  };
}

export type AnchorRect = Pick<DOMRect, "top" | "left" | "bottom" | "right" | "width" | "height">;

/** Place a fixed popover under (or above) an anchor, keeping it in the viewport. */
export function fixedPopoverStyle(
  anchor: AnchorRect,
  options: { width: number; estimatedHeight: number; gap?: number } = {
    width: 288,
    estimatedHeight: 280,
  },
): string {
  const gap = options.gap ?? 6;
  const preferBelow = anchor.bottom + gap;
  const spaceBelow = window.innerHeight - preferBelow;
  const top =
    spaceBelow < options.estimatedHeight && anchor.top > options.estimatedHeight
      ? Math.max(8, anchor.top - options.estimatedHeight - gap)
      : preferBelow;
  let left = anchor.left;
  if (left + options.width > window.innerWidth - 8) {
    left = Math.max(8, window.innerWidth - options.width - 8);
  }
  return `top:${top}px;left:${left}px;width:${options.width}px`;
}
