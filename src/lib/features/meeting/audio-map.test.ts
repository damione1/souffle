import { describe, it, expect } from "vitest";
import { buildPlayCommand, defaultAudioTarget, resolveAudioSeekTarget } from "./audio-map";
import type { MeetingAudioSession } from "../../types";

function audioSession(sessionIndex: number, path: string): MeetingAudioSession {
  return { session_index: sessionIndex, path, duration_seconds: null };
}

describe("resolveAudioSeekTarget", () => {
  it("returns null when the paragraph has no known recording session", () => {
    const result = resolveAudioSeekTarget(null, 12.5, [audioSession(0, "/rec/0.ogg")]);
    expect(result).toBeNull();
  });

  it("returns null when the session has no matching audio file", () => {
    const result = resolveAudioSeekTarget(2, 5, [audioSession(0, "/rec/0.ogg"), audioSession(1, "/rec/1.ogg")]);
    expect(result).toBeNull();
  });

  it("maps a paragraph to its session's file and offset", () => {
    const sessions = [audioSession(0, "/rec/0.ogg"), audioSession(1, "/rec/1.ogg")];
    const result = resolveAudioSeekTarget(1, 42.3, sessions);
    expect(result).toEqual({ path: "/rec/1.ogg", sessionIndex: 1, offsetSeconds: 42.3 });
  });

  it("clamps a negative start time to zero", () => {
    const result = resolveAudioSeekTarget(0, -0.2, [audioSession(0, "/rec/0.ogg")]);
    expect(result?.offsetSeconds).toBe(0);
  });

  it("resolves the correct session in a multi-session meeting regardless of array order", () => {
    // audioSessions from get_meeting_audio is sorted by session_index, but
    // the mapping must not assume that ordering.
    const sessions = [audioSession(2, "/rec/2.ogg"), audioSession(0, "/rec/0.ogg"), audioSession(1, "/rec/1.ogg")];
    expect(resolveAudioSeekTarget(0, 1, sessions)?.path).toBe("/rec/0.ogg");
    expect(resolveAudioSeekTarget(1, 1, sessions)?.path).toBe("/rec/1.ogg");
    expect(resolveAudioSeekTarget(2, 1, sessions)?.path).toBe("/rec/2.ogg");
  });

  it("handles a meeting where only the second session was recorded", () => {
    // e.g. retention was turned on between the first and second recording.
    const sessions = [audioSession(1, "/rec/1.ogg")];
    expect(resolveAudioSeekTarget(0, 1, sessions)).toBeNull();
    expect(resolveAudioSeekTarget(1, 7, sessions)).toEqual({
      path: "/rec/1.ogg",
      sessionIndex: 1,
      offsetSeconds: 7,
    });
  });
});

describe("defaultAudioTarget", () => {
  it("returns null when there are no recorded sessions", () => {
    expect(defaultAudioTarget([])).toBeNull();
  });

  it("picks the earliest session at offset zero, regardless of input order", () => {
    const sessions = [audioSession(2, "/rec/2.ogg"), audioSession(0, "/rec/0.ogg"), audioSession(1, "/rec/1.ogg")];
    expect(defaultAudioTarget(sessions)).toEqual({ path: "/rec/0.ogg", sessionIndex: 0, offsetSeconds: 0 });
  });

  it("works for a single-session meeting", () => {
    expect(defaultAudioTarget([audioSession(0, "/rec/0.ogg")])).toEqual({
      path: "/rec/0.ogg",
      sessionIndex: 0,
      offsetSeconds: 0,
    });
  });
});

describe("buildPlayCommand", () => {
  it("flags a session change when the target's file differs from the current one", () => {
    const target = { path: "/rec/1.ogg", sessionIndex: 1, offsetSeconds: 10 };
    const command = buildPlayCommand(target, "/rec/0.ogg");
    expect(command).toEqual({ path: "/rec/1.ogg", seekSeconds: 10, sessionChanged: true });
  });

  it("does not flag a session change for another paragraph in the same file", () => {
    const target = { path: "/rec/0.ogg", sessionIndex: 0, offsetSeconds: 33 };
    const command = buildPlayCommand(target, "/rec/0.ogg");
    expect(command).toEqual({ path: "/rec/0.ogg", seekSeconds: 33, sessionChanged: false });
  });

  it("flags a session change when there was no current path yet", () => {
    const target = { path: "/rec/0.ogg", sessionIndex: 0, offsetSeconds: 0 };
    const command = buildPlayCommand(target, null);
    expect(command.sessionChanged).toBe(true);
  });
});
