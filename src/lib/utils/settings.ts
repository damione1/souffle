import { invoke } from "@tauri-apps/api/core";
import type { AppSettings, Theme } from "../types";

export type LoadedSettings = Partial<AppSettings> & { audio_device?: string };

/** Load settings from the SQLite database via Tauri commands */
export async function loadSettingsFromDb(): Promise<LoadedSettings> {
  const raw = await invoke<Record<string, unknown>>("get_settings");
  const result: LoadedSettings = {};

  if (typeof raw.theme === "string") {
    result.theme = raw.theme as Theme;
  }
  if (typeof raw.auto_paste === "boolean") {
    result.auto_paste = raw.auto_paste;
  }
  if (typeof raw.paste_delay_ms === "number") {
    result.paste_delay_ms = raw.paste_delay_ms;
  }
  if (typeof raw.debug_transcription === "boolean") {
    result.debug_transcription = raw.debug_transcription;
  }
  if (typeof raw.ollama_url === "string") {
    result.ollama_url = raw.ollama_url;
  }
  if (typeof raw.ollama_model === "string") {
    result.ollama_model = raw.ollama_model;
  }
  if (typeof raw.audio_device === "string") {
    result.audio_device = raw.audio_device;
  }

  return result;
}
