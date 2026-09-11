import { describe, it, expect } from "vitest";
import type { SnippetEntry } from "../../types";
import { applySnippet, foldSnippetTrigger } from "./snippets";

function snippet(id: number, trigger: string, expansion: string): SnippetEntry {
  return { id, trigger, expansion, created_at: "" };
}

describe("foldSnippetTrigger", () => {
  it("drops case and accents", () => {
    expect(foldSnippetTrigger("Signature Mail")).toBe("signature mail");
    expect(foldSnippetTrigger("Résumé")).toBe("resume");
  });

  it("gives decomposed and precomposed input the same key", () => {
    expect(foldSnippetTrigger("re\u0301sume\u0301")).toBe(foldSnippetTrigger("résumé"));
  });
});

describe("applySnippet", () => {
  const signature = snippet(1, "signature mail", "Cordialement,\nDamien");

  it("returns null with no snippets", () => {
    expect(applySnippet("Signature mail", [])).toBeNull();
  });

  it("returns null when no trigger matches", () => {
    expect(applySnippet("Bonjour à tous", [signature])).toBeNull();
  });

  it("returns null when the trigger only matches mid-text", () => {
    expect(applySnippet("Ma signature mail est prête", [signature])).toBeNull();
  });

  it("replaces the trigger and keeps the rest of the transcript", () => {
    expect(applySnippet("Signature mail, et à bientôt.", [signature])).toBe(
      "Cordialement,\nDamien, et à bientôt.",
    );
    expect(applySnippet("Signature mail et à bientôt", [signature])).toBe(
      "Cordialement,\nDamien et à bientôt",
    );
  });

  it("returns only the expansion when the trigger is the whole transcript", () => {
    expect(applySnippet("Signature mail", [signature])).toBe("Cordialement,\nDamien");
  });

  it("requires a word boundary after the trigger", () => {
    const brb = snippet(2, "brb", "be right back");
    expect(applySnippet("brbx", [brb])).toBeNull();
    expect(applySnippet("brb2", [brb])).toBeNull();
    expect(applySnippet("brb, see you", [brb])).toBe("be right back, see you");
    expect(applySnippet("brb.", [brb])).toBe("be right back.");
  });

  it("prefers the longest matching trigger regardless of list order", () => {
    const short = snippet(1, "signature", "SHORT");
    const long = snippet(2, "signature mail", "LONG");
    expect(applySnippet("Signature mail pro", [short, long])).toBe("LONG pro");
    expect(applySnippet("Signature mail pro", [long, short])).toBe("LONG pro");
    expect(applySnippet("Signature pro", [long, short])).toBe("SHORT pro");
  });

  it("matches case- and accent-insensitively in both directions", () => {
    const resume = snippet(1, "resume", "Résumé de la réunion :");
    const cafe = snippet(2, "Café", "Café de la Paix");
    expect(applySnippet("Résumé bonjour", [resume])).toBe("Résumé de la réunion : bonjour");
    expect(applySnippet("RÉSUMÉ", [resume])).toBe("Résumé de la réunion :");
    expect(applySnippet("cafe demain", [cafe])).toBe("Café de la Paix demain");
  });

  it("cuts the remainder at the matched prefix on decomposed input", () => {
    const resume = snippet(1, "resume", "R");
    // "résumé bonjour" in NFD: the 6-letter word is 8 code units long, so a
    // `trigger.length` slice would leave the last accent in the remainder.
    const decomposed = "re\u0301sume\u0301 bonjour";
    expect(decomposed.length).toBe(16);
    expect(applySnippet(decomposed, [resume])).toBe("R bonjour");
    // Mirror case: a decomposed trigger against precomposed text.
    const decomposedTrigger = snippet(2, "re\u0301sume\u0301", "R");
    expect(applySnippet("Résumé bonjour", [decomposedTrigger])).toBe("R bonjour");
  });

  it("ignores blank triggers", () => {
    expect(applySnippet("hello", [snippet(1, "   ", "X")])).toBeNull();
  });
});
