import { describe, expect, it } from "vitest";
import { toAppSettings, withAudioDevice } from "./settings";
import type { PersistedAppSettings } from "../types";

describe("settings helpers", () => {
  const persistedSettings: PersistedAppSettings = {
    theme: "dark",
    auto_paste: true,
    paste_delay_ms: 150,
    ollama_url: "http://localhost:11434",
    ollama_model: "qwen2.5",
    debug_transcription: false,
    audio_device: "BlackHole",
  };

  it("removes audio_device when converting to app settings", () => {
    expect(toAppSettings(persistedSettings)).toEqual({
      theme: "dark",
      auto_paste: true,
      paste_delay_ms: 150,
      ollama_url: "http://localhost:11434",
      ollama_model: "qwen2.5",
      debug_transcription: false,
    });
  });

  it("adds audio_device when building persisted settings", () => {
    expect(withAudioDevice(toAppSettings(persistedSettings), "BlackHole")).toEqual(
      persistedSettings,
    );
  });

  it("supports a null audio device", () => {
    expect(withAudioDevice(toAppSettings(persistedSettings), null).audio_device).toBeNull();
  });
});
