import { describe, expect, it, vi } from "vitest";
import { HAVE_METADATA, applyPendingSeek, seekNeedsMetadata } from "./audio-seek";

function media(readyState: number, currentTime = 0, src = "/rec/0.ogg") {
  return {
    readyState,
    currentTime,
    src,
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
    const leftover = applyPendingSeek(el, { seekSeconds: 12.5, src: "/rec/0.ogg" });
    expect(el.currentTime).toBe(0);
    expect(el.play).not.toHaveBeenCalled();
    expect(leftover).toEqual({ seekSeconds: 12.5, src: "/rec/0.ogg" });
  });

  it("applies the pending offset once metadata is available", () => {
    const el = media(HAVE_METADATA);
    const leftover = applyPendingSeek(el, { seekSeconds: 12.5, src: "/rec/0.ogg" });
    expect(el.currentTime).toBe(12.5);
    expect(el.play).toHaveBeenCalledOnce();
    expect(leftover).toBeNull();
  });

  it("a later pending seek replaces the earlier one", () => {
    const el = media(0);
    applyPendingSeek(el, { seekSeconds: 4, src: "/rec/0.ogg" });
    el.readyState = HAVE_METADATA;
    const leftover = applyPendingSeek(el, { seekSeconds: 18, src: "/rec/0.ogg" });
    expect(el.currentTime).toBe(18);
    expect(leftover).toBeNull();
  });

  it("does not apply a later file's offset to a stale metadata event", () => {
    const el = media(HAVE_METADATA, 0, "/rec/b.ogg");
    const leftover = applyPendingSeek(el, { seekSeconds: 20, src: "/rec/c.ogg" });
    expect(el.currentTime).toBe(0);
    expect(el.play).not.toHaveBeenCalled();
    expect(leftover).toEqual({ seekSeconds: 20, src: "/rec/c.ogg" });
  });
});
