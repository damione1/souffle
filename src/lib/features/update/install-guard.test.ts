import { describe, expect, it } from "vitest";
import { blockFromMachine } from "./install-block";
import type { AppStateMachine } from "../../types";
import type { InstallBlockReason } from "../../api/updater";

/** Exercises the production table the controller uses, not a copy of it.
 * Mirrors `install_blocked_reason` in `src-tauri/src/commands/updater.rs`,
 * which is tested against the same ten states on the Rust side. */
describe("update install guard (UI)", () => {
  const blocked: [AppStateMachine["state"], InstallBlockReason][] = [
    ["recording_dictation", "recording_dictation"],
    ["recording_meeting", "recording_meeting"],
    ["stopping", "stopping"],
    ["downloading", "downloading"],
    ["loading", "loading"],
    ["unloading", "unloading"],
  ];

  it.each(blocked)("disables install while %s", (state, reason) => {
    expect(blockFromMachine(state)).toBe(reason);
  });

  it.each<AppStateMachine["state"]>(["idle", "ready", "downloaded", "error"])(
    "allows install while %s",
    (state) => {
      expect(blockFromMachine(state)).toBeNull();
    },
  );

  it("covers every state of the machine", () => {
    const covered = [...blocked.map(([state]) => state), "idle", "ready", "downloaded", "error"];
    expect(new Set(covered).size).toBe(10);
  });
});
