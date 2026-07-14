import { describe, expect, it } from "vitest";
import { isClickableTranscriptWord, tokenizeTranscriptWords } from "./transcript-words";

describe("tokenizeTranscriptWords", () => {
  it("splits words and preserves gaps", () => {
    expect(tokenizeTranscriptWords("Hello, café-théâtre!")).toEqual([
      { kind: "word", text: "Hello" },
      { kind: "gap", text: ", " },
      { kind: "word", text: "café-théâtre" },
      { kind: "gap", text: "!" },
    ]);
  });

  it("handles apostrophes inside words", () => {
    expect(tokenizeTranscriptWords("c'est bien")).toEqual([
      { kind: "word", text: "c'est" },
      { kind: "gap", text: " " },
      { kind: "word", text: "bien" },
    ]);
  });

  it("returns an empty list for blank text", () => {
    expect(tokenizeTranscriptWords("")).toEqual([]);
    expect(tokenizeTranscriptWords("   ")).toEqual([{ kind: "gap", text: "   " }]);
  });

  it("gives duplicate words distinct indices for popover targeting", () => {
    const tokens = tokenizeTranscriptWords("Alex met Alex");
    const wordIndexes = tokens.flatMap((token, index) =>
      token.kind === "word" && token.text === "Alex" ? [index] : [],
    );
    expect(wordIndexes).toEqual([0, 4]);
  });
});

describe("isClickableTranscriptWord", () => {
  it("accepts names and rejects one-letter tokens", () => {
    expect(isClickableTranscriptWord("JF")).toBe(true);
    expect(isClickableTranscriptWord("a")).toBe(false);
    expect(isClickableTranscriptWord("42")).toBe(false);
  });
});
