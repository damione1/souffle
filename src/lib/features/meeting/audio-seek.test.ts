import { describe, expect, it, vi } from "vitest";
import { HAVE_METADATA, applyPendingSeek, seekNeedsMetadata } from "./audio-seek";

function media(readyState: number, currentTime = 0) {
  return {
    readyState,
    currentTime,
    play: vi.fn(() => undefined),
  };
}

describe("seekNeedsMetadata", () => {
  it("waits when the same file has no metadata yet", () => {
    expect(seekNeedsMetadata(0, false)).toBe(true);
  });

  it("applies immediately when the same file already has metadata", () => {
    expect(seekNeedsMetadata(HAVE_METADATA, false)).toBe(false);
  });

  it("waits on a session swap even if the current element already has metadata", () => {
    expect(seekNeedsMetadata(HAVE_METADATA, true)).toBe(true);
  });
});

describe("applyPendingSeek", () => {
  it("does not set currentTime before metadata", () => {
    const el = media(0);
    const leftover = applyPendingSeek(el, { seekSeconds: 12.5 });
    expect(el.currentTime).toBe(0);
    expect(el.play).not.toHaveBeenCalled();
    expect(leftover).toEqual({ seekSeconds: 12.5 });
  });

  it("applies the pending offset once metadata is available", () => {
    const el = media(HAVE_METADATA);
    const leftover = applyPendingSeek(el, { seekSeconds: 12.5 });
    expect(el.currentTime).toBe(12.5);
    expect(el.play).toHaveBeenCalledOnce();
    expect(leftover).toBeNull();
  });

  it("a later pending seek replaces the earlier one", () => {
    const el = media(0);
    applyPendingSeek(el, { seekSeconds: 4 });
    el.readyState = HAVE_METADATA;
    const leftover = applyPendingSeek(el, { seekSeconds: 18 });
    expect(el.currentTime).toBe(18);
    expect(leftover).toBeNull();
  });
});
