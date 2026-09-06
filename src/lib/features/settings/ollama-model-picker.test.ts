import { describe, expect, it } from "vitest";
import { ollamaModelPickerState } from "./ollama-model-picker";

describe("ollamaModelPickerState", () => {
  it("shows a muted fallback picker under Auto when Apple Intelligence is available", () => {
    expect(ollamaModelPickerState("auto", true, 2)).toEqual({
      visible: true,
      muted: true,
      showFallbackHint: true,
    });
  });

  it("enables the picker under Auto when Apple Intelligence is unavailable", () => {
    expect(ollamaModelPickerState("auto", false, 2)).toEqual({
      visible: true,
      muted: false,
      showFallbackHint: false,
    });
  });

  it("enables the picker when the provider is locked to Ollama", () => {
    expect(ollamaModelPickerState("ollama", true, 1)).toEqual({
      visible: true,
      muted: false,
      showFallbackHint: false,
    });
  });

  it("hides the picker when the provider is locked to Apple Intelligence", () => {
    expect(ollamaModelPickerState("apple_intelligence", true, 2)).toEqual({
      visible: false,
      muted: false,
      showFallbackHint: false,
    });
  });

  it("hides the picker when there are no compatible models", () => {
    expect(ollamaModelPickerState("auto", true, 0).visible).toBe(false);
    expect(ollamaModelPickerState("ollama", false, 0).visible).toBe(false);
  });
});
