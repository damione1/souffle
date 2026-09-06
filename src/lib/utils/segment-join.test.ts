import { describe, it, expect } from "vitest";

import { segmentGap } from "./segment-join";

describe("segmentGap", () => {
  it("adds a space when neither side has one", () => {
    expect(segmentGap("hello", "world")).toBe(" ");
  });

  it("adds nothing when the text already ends with a space", () => {
    expect(segmentGap("hello ", "world")).toBe("");
  });

  it("adds nothing when the segment already starts with a space", () => {
    expect(segmentGap("hello", " world")).toBe("");
  });

  it("adds nothing when both sides have a space", () => {
    expect(segmentGap("hello ", " world")).toBe("");
  });

  it("adds nothing at the start of a transcript", () => {
    expect(segmentGap("", "hello")).toBe("");
  });

  it("adds nothing for an empty segment", () => {
    expect(segmentGap("hello", "")).toBe("");
  });
});
