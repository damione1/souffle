import { invoke } from "@tauri-apps/api/core";
import type {
  AppSettings,
  AudioDevice,
  PersistedAppSettings,
  ShortcutSettings,
} from "../types";

export async function getSettings(): Promise<PersistedAppSettings> {
  return invoke<PersistedAppSettings>("get_settings");
}

export async function saveSettings(settings: PersistedAppSettings): Promise<void> {
  await invoke("save_settings", { settings });
}

export async function getShortcuts(): Promise<ShortcutSettings> {
  return invoke<ShortcutSettings>("get_shortcuts");
}

export async function saveShortcuts(shortcuts: ShortcutSettings): Promise<void> {
  await invoke("save_shortcuts", { shortcuts });
}

export async function listAudioDevices(): Promise<AudioDevice[]> {
  return invoke<AudioDevice[]>("list_audio_devices");
}

export async function selectAudioDevice(deviceName: string): Promise<void> {
  await invoke("select_audio_device", { deviceName });
}

export function toAppSettings(settings: PersistedAppSettings): AppSettings {
  const { audio_device, ...appSettings } = settings;
  return appSettings;
}

export function withAudioDevice(
  settings: AppSettings,
  audioDevice: string | null,
): PersistedAppSettings {
  return {
    ...settings,
    audio_device: audioDevice,
  };
}
