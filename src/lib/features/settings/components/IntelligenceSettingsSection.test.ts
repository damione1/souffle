import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/svelte";
import IntelligenceSettingsSection from "./IntelligenceSettingsSection.svelte";
import type { SummaryModelDescriptor, SummaryProviderChoice } from "../../../types";

const llama: SummaryModelDescriptor = {
  id: "llama3",
  label: "Llama 3",
  provider: "ollama",
  can_summarize: true,
};
const whisper: SummaryModelDescriptor = {
  id: "whisper",
  label: "Whisper",
  provider: "ollama",
  can_summarize: false,
};

function renderSection(overrides: {
  appleIntelligenceAvailable?: boolean;
  summaryProvider?: SummaryProviderChoice;
  summaryModels?: SummaryModelDescriptor[];
  ollamaModels?: SummaryModelDescriptor[];
} = {}) {
  const summaryModels = overrides.summaryModels ?? [llama];
  return render(IntelligenceSettingsSection, {
    props: {
      ollamaUrl: "http://localhost:11434",
      ollamaAvailable: true,
      appleIntelligenceAvailable: overrides.appleIntelligenceAvailable ?? true,
      appleIntelligenceUnavailableReason: null,
      ollamaModels: overrides.ollamaModels ?? [llama, whisper],
      summaryModels,
      summaryProvider: overrides.summaryProvider ?? "auto",
      onSummaryProviderChange: vi.fn(),
      selectedOllamaModel: "llama3",
      recommendedOllamaModel: "qwen2.5:7b",
      ollamaPulling: false,
      ollamaPullStatus: "",
      ollamaPullDownloaded: 0,
      ollamaPullTotal: null,
      ollamaPullError: "",
      onOllamaUrlChange: vi.fn(),
      onOllamaModelChange: vi.fn(),
      onRetrySummaryProviders: vi.fn(),
      onDownloadRecommendedOllamaModel: vi.fn(),
    },
  });
}

describe("IntelligenceSettingsSection", () => {
  it("shows a muted, still-usable fallback picker under Auto when Apple is available", () => {
    const { container } = renderSection();

    const select = screen.getByLabelText("Summarization model") as HTMLSelectElement;
    expect(select.disabled).toBe(false);
    expect(select.value).toBe("llama3");
    expect(screen.getByText("Used if Apple Intelligence becomes unavailable.")).toBeTruthy();
    expect(container.querySelector(".opacity-50")).toBeTruthy();
  });

  it("enables the picker without a fallback caption when Ollama is the provider", () => {
    const { container } = renderSection({
      summaryProvider: "ollama",
      appleIntelligenceAvailable: true,
    });

    const select = screen.getByLabelText("Summarization model") as HTMLSelectElement;
    expect(select.disabled).toBe(false);
    expect(screen.queryByText("Used if Apple Intelligence becomes unavailable.")).toBeNull();
    expect(container.querySelector(".opacity-50")).toBeNull();
  });

  it("hides the picker when Apple Intelligence is locked in", () => {
    renderSection({ summaryProvider: "apple_intelligence" });

    expect(screen.queryByLabelText("Summarization model")).toBeNull();
  });

  it("counts compatible models in the connection status, not every Ollama model", () => {
    renderSection({
      ollamaModels: [llama, whisper],
      summaryModels: [llama],
    });

    expect(screen.getByText("1 compatible model found.")).toBeTruthy();
    expect(screen.queryByText("2 models found.")).toBeNull();
  });
});
