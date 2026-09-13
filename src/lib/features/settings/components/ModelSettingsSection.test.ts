import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/svelte";
import type { TranscriptionCatalog } from "../../../types";
import ModelSettingsSection from "./ModelSettingsSection.svelte";

const catalog: TranscriptionCatalog = {
  engines: [
    {
      id: "kyutai",
      label: "Kyutai",
      description: "Kyutai STT",
      models: [
        {
          id: "stt-1b-en_fr",
          label: "STT 1B",
          description: "1B",
          download_size_bytes: 1,
          recommended_memory_bytes: 1,
          supported_languages: ["en"],
          capabilities: {
            supports_streaming: true,
            supports_batch_transcription: false,
            supports_language_auto_detect: true,
            supports_word_timestamps: true,
            supports_partial_results: true,
          },
          audio_input: { sample_rate_hz: 24000, channels: 1, chunk_size_samples: 1920 },
          available_in_app: true,
          availability_note: null,
          backends: [
            {
              id: "candle",
              label: "Candle",
              description: "Candle",
              recommended: true,
              available_in_app: true,
              availability_note: null,
              artifacts: [],
            },
          ],
          recommended_backend_id: "candle",
        },
      ],
    },
  ],
  selected_engine_id: "kyutai",
  selected_model_id: "stt-1b-en_fr",
  selected_backend_id: "candle",
};

function renderSection(recording: boolean) {
  render(ModelSettingsSection, {
    props: {
      catalog,
      // What `get_settings_options` serves.
      settingsOptions: {
        model_unload_timeout_minutes: [0, 5, 15, 60],
        meeting_autostop_minutes: [5, 10, 15, 30],
        meeting_max_duration_minutes: [120, 240, 480],
      },
      selectedEngineId: "kyutai",
      selectedModelId: "stt-1b-en_fr",
      runtimePhase: "ready",
      operationState: "idle",
      downloadedBytes: 0,
      downloadTotalBytes: null,
      downloadFile: "",
      unloadTimeoutMinutes: 0,
      recording,
      onSelectModel: vi.fn(),
      onUnloadTimeoutChange: vi.fn(),
    },
  });
}

describe("ModelSettingsSection recording guard", () => {
  afterEach(cleanup);

  it("disables the model picker while recording", () => {
    renderSection(true);
    const picker = screen.getByLabelText("Transcription model") as HTMLSelectElement;
    expect(picker.disabled).toBe(true);
    expect(picker.title).toBe("Can't change the transcription model while recording.");
  });

  it("leaves the picker enabled when idle", () => {
    renderSection(false);
    const picker = screen.getByLabelText("Transcription model") as HTMLSelectElement;
    expect(picker.disabled).toBe(false);
  });
});
