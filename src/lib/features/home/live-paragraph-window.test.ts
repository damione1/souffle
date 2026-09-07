import { describe, expect, it } from "vitest";
import {
  LIVE_PARAGRAPH_WINDOW,
  leadingRemovedCount,
  measureLeadingHeight,
  scrollTopAfterLeadingUnmount,
  windowedParagraphs,
} from "./live-paragraph-window";

function ids(count: number, start = 0): number[] {
  return Array.from({ length: count }, (_, i) => start + i);
}

describe("windowedParagraphs", () => {
  it("keeps the last 30 committed paragraphs plus tail, capped at 30", () => {
    const committed = ids(40).map((id) => ({ id }));
    const tail = [{ id: 40 }];
    const windowed = windowedParagraphs(committed, tail);
    expect(windowed).toHaveLength(LIVE_PARAGRAPH_WINDOW);
    expect(windowed[0].id).toBe(11);
    expect(windowed.at(-1)?.id).toBe(40);
  });

  it("does not mount the full transcript under the cap", () => {
    const committed = ids(80).map((id) => ({ id }));
    expect(windowedParagraphs(committed, []).map((item) => item.id)).toEqual(ids(30, 50));
  });
});

describe("live window scroll compensation", () => {
  const paraHeight = 50;
  const gap = 16;
  const stride = paraHeight + gap;

  function idAtScrollTop(paragraphIds: number[], scrollTop: number): number {
    const index = Math.min(paragraphIds.length - 1, Math.floor(scrollTop / stride));
    return paragraphIds[index];
  }

  it("keeps the paragraph under the viewport top when the 31st commit unmounts the oldest", () => {
    const previous = windowedParagraphs(ids(30), []);
    const next = windowedParagraphs(ids(31), []);
    expect(next).toHaveLength(30);
    expect(leadingRemovedCount(previous, next)).toBe(1);

    const scrollTop = 8 * stride + 10;
    const beforeId = idAtScrollTop(previous, scrollTop);
    const compensated = scrollTopAfterLeadingUnmount(scrollTop, stride);
    expect(idAtScrollTop(next, compensated)).toBe(beforeId);
  });

  it("treats a fully replaced 30-item window as 30 leading unmounts", () => {
    const previous = windowedParagraphs(ids(30), []);
    const next = windowedParagraphs(ids(30, 100), []);
    expect(next[0]).not.toBe(previous[0]);
    expect(leadingRemovedCount(previous, next)).toBe(LIVE_PARAGRAPH_WINDOW);
  });

  it("leaves a bottom-stuck viewport to autoscroll (compensation is a no-op at 0 unmounts)", () => {
    const previous = windowedParagraphs(ids(10), []);
    const next = windowedParagraphs(ids(11), []);
    expect(leadingRemovedCount(previous, next)).toBe(0);
    expect(scrollTopAfterLeadingUnmount(400, 0)).toBe(400);
  });

  it("clamps compensated scrollTop at 0", () => {
    expect(scrollTopAfterLeadingUnmount(10, 40)).toBe(0);
  });

  it("measures the height of leading children including gap", () => {
    const children = [
      { offsetHeight: 50 },
      { offsetHeight: 60 },
      { offsetHeight: 70 },
    ];
    const container = { children } as unknown as HTMLElement;
    expect(measureLeadingHeight(container, 2, 16)).toBe(50 + 16 + 60 + 16);
  });
});
