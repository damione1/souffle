import type { SettingsTab } from "./open";

/** Deep-link targets inside Settings, each mapped to the tab that renders it.
 *
 * This registry is the single source of truth: callers name an anchor, never a
 * tab, so a target that no longer exists is a type error instead of a sentence
 * that quietly stops being true (SOU-056). `anchors.test.ts` additionally fails
 * when a declared anchor is not placed anywhere in the settings markup. */
export const SETTINGS_ANCHORS = {
  "transcription.model": "transcription",
  "ai.provider": "ai",
  "ai.ollama_model": "ai",
  "ai.dictation_polish": "ai",
  "audio.mic": "audio",
  "interface.shortcuts": "interface",
  "meetings.calendar": "meetings",
  "system.permissions": "system",
} as const satisfies Record<string, SettingsTab>;

export type SettingsAnchor = keyof typeof SETTINGS_ANCHORS;

export function tabForAnchor(anchor: SettingsAnchor): SettingsTab {
  return SETTINGS_ANCHORS[anchor];
}

/** Transient accent ring applied on arrival. Defined in `app.css`, which drops
 * the fade (not the ring) under `prefers-reduced-motion`. */
export const ANCHOR_HIGHLIGHT_CLASS = "settings-anchor-highlight";
export const ANCHOR_HIGHLIGHT_MS = 2200;

export function prefersReducedMotion(): boolean {
  return typeof window !== "undefined"
    && typeof window.matchMedia === "function"
    && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/** Scroll the anchored row into view and mark it for a couple of seconds.
 * Returns a disposer that cancels the pending removal, or null when the anchor
 * is not on screen (wrong tab, or a section hidden behind a condition). */
export function focusAnchor(
  anchor: SettingsAnchor,
  options: { root?: Document | HTMLElement; reducedMotion?: boolean } = {},
): (() => void) | null {
  const root = options.root ?? (typeof document !== "undefined" ? document : null);
  if (!root) return null;

  const element = root.querySelector<HTMLElement>(`[data-settings-anchor="${anchor}"]`);
  if (!element) return null;

  const reducedMotion = options.reducedMotion ?? prefersReducedMotion();
  // jsdom (and any non-browser host) has no scrollIntoView; the highlight is
  // what the test asserts on, the scroll is best-effort.
  element.scrollIntoView?.({ block: "center", behavior: reducedMotion ? "auto" : "smooth" });
  element.classList.add(ANCHOR_HIGHLIGHT_CLASS);

  const timer = setTimeout(() => {
    element.classList.remove(ANCHOR_HIGHLIGHT_CLASS);
  }, ANCHOR_HIGHLIGHT_MS);

  return () => {
    clearTimeout(timer);
    element.classList.remove(ANCHOR_HIGHLIGHT_CLASS);
  };
}
