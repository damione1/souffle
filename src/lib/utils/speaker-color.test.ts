import { describe, expect, it } from "vitest";
import { persistentSpeakerId, speakerPaletteIndex, speakerPillClass, SPEAKER_PALETTE_SIZE } from "./speaker-color";

describe("speakerPaletteIndex", () => {
  it("is stable for the same id", () => {
    expect(speakerPaletteIndex(7)).toBe(speakerPaletteIndex(7));
  });

  it("stays within the palette", () => {
    for (const id of [1, 2, 42, 999, 12345]) {
      const index = speakerPaletteIndex(id);
      expect(index).toBeGreaterThanOrEqual(0);
      expect(index).toBeLessThan(SPEAKER_PALETTE_SIZE);
    }
  });
});

describe("speakerPillClass", () => {
  it("maps to a speaker-pill class", () => {
    expect(speakerPillClass(3)).toMatch(/^speaker-pill-\d+$/);
  });
});

describe("persistentSpeakerId", () => {
  it("parses spk labels", () => {
    expect(persistentSpeakerId("spk:12")).toBe(12);
    expect(persistentSpeakerId("me")).toBeNull();
    expect(persistentSpeakerId(null)).toBeNull();
  });
});
