import { describe, expect, it } from "vitest";
import type { TranscriptionSegment } from "../../types";
import { redistributeSegmentTexts } from "./live-edit";

function seg(text: string, speaker: "me" | "them"): TranscriptionSegment {
  return {
    text,
    start_time: 0,
    end_time: 0.5,
    is_final: true,
    language: null,
    confidence: null,
    speaker,
  };
}

describe("redistributeSegmentTexts", () => {
  it("patches only the listed emission indices of a crosstalk Me turn", () => {
    const segments = [
      seg("hello", "me"),
      seg("hi", "them"),
      seg("how", "me"),
      seg("good", "them"),
      seg("are", "me"),
      seg("thanks", "them"),
      seg("you", "me"),
    ];

    redistributeSegmentTexts(segments, [0, 2, 4, 6], "hello how are we");

    expect(segments.map((s) => s.text)).toEqual([
      "hello",
      "hi",
      "how",
      "good",
      "are",
      "thanks",
      "we",
    ]);
  });

  it("leaves every other speaker intact when given a closed range that would have included them", () => {
    const segments = [
      seg("hello", "me"),
      seg("hi", "them"),
      seg("how", "me"),
    ];
    redistributeSegmentTexts(segments, [0, 2], "hello there");
    expect(segments[1].text).toBe("hi");
    expect(segments[0].text).toBe("hello");
    expect(segments[2].text).toBe("there");
  });
});
