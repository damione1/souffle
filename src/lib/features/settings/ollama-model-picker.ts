import type { SummaryProviderChoice } from "../../types";

export type OllamaModelPickerState = {
  visible: boolean;
  /**
   * Auto is on Apple Intelligence. Mute the row and show the fallback
   * caption so a model change does not look like it applies to polish.
   * The select itself stays enabled: otherwise there is still no way
   * to pick the fallback without switching provider to Ollama.
   */
  muted: boolean;
  showFallbackHint: boolean;
};

/**
 * Ollama's model picker belongs on screen whenever Ollama could run:
 * now, or as Auto's fallback. Locked Apple Intelligence hides it.
 */
export function ollamaModelPickerState(
  summaryProvider: SummaryProviderChoice,
  appleIntelligenceAvailable: boolean,
  summaryModelCount: number,
): OllamaModelPickerState {
  if (summaryModelCount === 0 || summaryProvider === "apple_intelligence") {
    return { visible: false, muted: false, showFallbackHint: false };
  }

  const fallbackOnly = summaryProvider === "auto" && appleIntelligenceAvailable;
  return {
    visible: true,
    muted: fallbackOnly,
    showFallbackHint: fallbackOnly,
  };
}
