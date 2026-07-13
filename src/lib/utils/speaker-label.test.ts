import { describe, it, expect } from "vitest";
import { resolveSpeakerLabel, speakerPlainLabel } from "./speaker-label";
import type { MeetingSpeaker } from "../types";

const speakers: MeetingSpeaker[] = [
  { id: 1, name: "Alice" },
  { id: 2, name: "Bob" },
];

describe("resolveSpeakerLabel", () => {
  it("returns null for no speaker", () => {
    expect(resolveSpeakerLabel(null, speakers)).toBeNull();
    expect(resolveSpeakerLabel(undefined, speakers)).toBeNull();
  });

  it("resolves me and them", () => {
    expect(resolveSpeakerLabel("me", speakers)).toEqual({ kind: "me" });
    expect(resolveSpeakerLabel("them", speakers)).toEqual({ kind: "them" });
  });

  it("resolves a persistent speaker id to its name", () => {
    expect(resolveSpeakerLabel("spk:1", speakers)).toEqual({ kind: "named", name: "Alice" });
    expect(resolveSpeakerLabel("spk:2", speakers)).toEqual({ kind: "named", name: "Bob" });
  });

  it("falls back to unknown when the id isn't in the speakers list", () => {
    expect(resolveSpeakerLabel("spk:99", speakers)).toEqual({ kind: "unknown", id: 99 });
    expect(resolveSpeakerLabel("spk:1", [])).toEqual({ kind: "unknown", id: 1 });
  });

  it("returns null for a malformed label", () => {
    expect(resolveSpeakerLabel("garbage", speakers)).toBeNull();
    expect(resolveSpeakerLabel("spk:abc", speakers)).toBeNull();
  });
});

describe("speakerPlainLabel", () => {
  it("mirrors the Rust exporters' Me/Them/name/Speaker <id> convention", () => {
    expect(speakerPlainLabel("me", speakers)).toBe("Me");
    expect(speakerPlainLabel("them", speakers)).toBe("Them");
    expect(speakerPlainLabel("spk:1", speakers)).toBe("Alice");
    expect(speakerPlainLabel("spk:99", speakers)).toBe("Speaker 99");
    expect(speakerPlainLabel(null, speakers)).toBeNull();
  });
});
