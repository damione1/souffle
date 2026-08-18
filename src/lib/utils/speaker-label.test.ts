import { describe, it, expect } from "vitest";
import { resolveSpeakerLabel, speakerPlainLabel } from "./speaker-label";

describe("resolveSpeakerLabel", () => {
  it("returns null for no speaker", () => {
    expect(resolveSpeakerLabel(null)).toBeNull();
    expect(resolveSpeakerLabel(undefined)).toBeNull();
  });

  it("resolves me and them", () => {
    expect(resolveSpeakerLabel("me")).toEqual({ kind: "me" });
    expect(resolveSpeakerLabel("them")).toEqual({ kind: "them" });
  });

  it("returns null for leftover persistent labels and garbage", () => {
    expect(resolveSpeakerLabel("spk:1")).toBeNull();
    expect(resolveSpeakerLabel("spk:abc")).toBeNull();
    expect(resolveSpeakerLabel("garbage")).toBeNull();
  });
});

describe("speakerPlainLabel", () => {
  it("mirrors the Rust exporters' Me/Them convention", () => {
    expect(speakerPlainLabel("me")).toBe("Me");
    expect(speakerPlainLabel("them")).toBe("Them");
    expect(speakerPlainLabel("spk:1")).toBeNull();
    expect(speakerPlainLabel(null)).toBeNull();
  });
});
