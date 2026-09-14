import { describe, it, expect } from "vitest";
import {
  SPEAKERS,
  isSpeaker,
  resolveSpeaker,
  speakerI18nKey,
  speakerPlainLabel,
  speakerTextClass,
} from "./speaker-label";

describe("SPEAKERS", () => {
  it("enumerates the generated union", () => {
    expect([...SPEAKERS].sort()).toEqual(["me", "them"]);
  });
});

describe("resolveSpeaker", () => {
  it("returns null for no speaker", () => {
    expect(resolveSpeaker(null)).toBeNull();
    expect(resolveSpeaker(undefined)).toBeNull();
  });

  it("resolves me and them", () => {
    expect(resolveSpeaker("me")).toBe("me");
    expect(resolveSpeaker("them")).toBe("them");
  });

  it("returns null for leftover persistent labels and garbage", () => {
    expect(resolveSpeaker("spk:1")).toBeNull();
    expect(resolveSpeaker("spk:abc")).toBeNull();
    expect(resolveSpeaker("garbage")).toBeNull();
    expect(resolveSpeaker("")).toBeNull();
  });

  it("does not resolve inherited object properties", () => {
    expect(isSpeaker("toString")).toBe(false);
    expect(isSpeaker("constructor")).toBe(false);
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

describe("badge lookups", () => {
  it("cover every variant of the union", () => {
    for (const speaker of SPEAKERS) {
      expect(speakerI18nKey(speaker)).toMatch(/^transcript\./);
      expect(speakerTextClass(speaker)).not.toBe("");
    }
  });
});
