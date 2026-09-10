import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  ANCHOR_HIGHLIGHT_CLASS,
  ANCHOR_HIGHLIGHT_MS,
  SETTINGS_ANCHORS,
  type SettingsAnchor,
  focusAnchor,
  tabForAnchor,
} from "./anchors";

/** Every settings component, as raw source: the registry is only useful if the
 * anchors it declares are actually placed in the markup. */
const sources = import.meta.glob<string>(
  ["/src/lib/features/settings/components/*.svelte", "/src/lib/components/**/*.svelte"],
  { query: "?raw", import: "default", eager: true },
);

describe("settings anchor registry", () => {
  it("maps every anchor to the tab named by its prefix", () => {
    for (const [anchor, tab] of Object.entries(SETTINGS_ANCHORS)) {
      expect(tabForAnchor(anchor as SettingsAnchor)).toBe(tab);
      expect(anchor.split(".")[0]).toBe(tab);
    }
  });

  /** SOU-089 AC1: a declared anchor with no home in the markup is exactly the
   * silent breakage the registry exists to prevent — the deep link would open
   * the right tab and highlight nothing. */
  it("places every declared anchor in the settings markup", () => {
    const markup = Object.values(sources).join("\n");
    const unplaced = Object.keys(SETTINGS_ANCHORS).filter(
      (anchor) =>
        !markup.includes(`anchor="${anchor}"`)
        && !markup.includes(`data-settings-anchor="${anchor}"`),
    );
    expect(unplaced).toEqual([]);
  });
});

describe("focusAnchor", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
    vi.useRealTimers();
  });

  function plant(anchor: string) {
    const el = document.createElement("div");
    el.setAttribute("data-settings-anchor", anchor);
    el.scrollIntoView = vi.fn();
    document.body.appendChild(el);
    return el;
  }

  it("scrolls the anchored row into view and highlights it", () => {
    const el = plant("audio.mic");

    focusAnchor("audio.mic", { reducedMotion: false });

    expect(el.scrollIntoView).toHaveBeenCalledWith({ block: "center", behavior: "smooth" });
    expect(el.classList.contains(ANCHOR_HIGHLIGHT_CLASS)).toBe(true);
  });

  it("drops the highlight after the transient window", () => {
    vi.useFakeTimers();
    const el = plant("ai.provider");

    focusAnchor("ai.provider", { reducedMotion: true });
    expect(el.classList.contains(ANCHOR_HIGHLIGHT_CLASS)).toBe(true);

    vi.advanceTimersByTime(ANCHOR_HIGHLIGHT_MS + 1);
    expect(el.classList.contains(ANCHOR_HIGHLIGHT_CLASS)).toBe(false);
  });

  /** AC4: reduced motion removes the animation, not the highlight — the row is
   * still marked, it just does not slide or fade into place. */
  it("still highlights, without smooth scrolling, under reduced motion", () => {
    const el = plant("system.permissions");

    focusAnchor("system.permissions", { reducedMotion: true });

    expect(el.scrollIntoView).toHaveBeenCalledWith({ block: "center", behavior: "auto" });
    expect(el.classList.contains(ANCHOR_HIGHLIGHT_CLASS)).toBe(true);
  });

  it("returns a disposer that clears the pending removal", () => {
    vi.useFakeTimers();
    const el = plant("meetings.calendar");

    const dispose = focusAnchor("meetings.calendar", { reducedMotion: true });
    dispose?.();

    expect(el.classList.contains(ANCHOR_HIGHLIGHT_CLASS)).toBe(false);
    vi.advanceTimersByTime(ANCHOR_HIGHLIGHT_MS + 1);
    expect(el.classList.contains(ANCHOR_HIGHLIGHT_CLASS)).toBe(false);
  });

  it("is a no-op when the anchor is not on screen", () => {
    expect(focusAnchor("interface.shortcuts", { reducedMotion: true })).toBeNull();
  });
});
