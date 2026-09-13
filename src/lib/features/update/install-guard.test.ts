import { describe, expect, it } from "vitest";

/** Mirrors `install_blocked_reason` / UpdateAction blocking set. */
const BLOCKING_STATES = new Set([
  "recording_dictation",
  "recording_meeting",
  "stopping",
  "downloading",
  "loading",
  "unloading",
]);

function isInstallBlocked(machineState: string): boolean {
  return BLOCKING_STATES.has(machineState);
}

describe("update install guard (UI)", () => {
  it.each([
    "recording_dictation",
    "recording_meeting",
    "stopping",
    "downloading",
    "loading",
    "unloading",
  ])("disables install while %s", (state) => {
    expect(isInstallBlocked(state)).toBe(true);
  });

  it.each(["idle", "ready", "downloaded", "error"])(
    "allows install while %s",
    (state) => {
      expect(isInstallBlocked(state)).toBe(false);
    },
  );
});
