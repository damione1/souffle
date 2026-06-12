import { commands, unwrap } from "./generated";
import type { AppSettings, AudioDeviceInfo, ShortcutSettings } from "../types";

export async function getSettings(): Promise<AppSettings> {
  return unwrap(commands.getSettings());
}

export async function saveSettings(settings: AppSettings): Promise<void> {
  await unwrap(commands.saveSettings(settings));
}

export async function getShortcuts(): Promise<ShortcutSettings> {
  return unwrap(commands.getShortcuts());
}

export async function saveShortcuts(shortcuts: ShortcutSettings): Promise<void> {
  await unwrap(commands.saveShortcuts(shortcuts));
}

export async function listAudioDevices(): Promise<AudioDeviceInfo[]> {
  return unwrap(commands.listAudioDevices());
}

export async function selectAudioDevice(deviceName: string): Promise<void> {
  await unwrap(commands.selectAudioDevice(deviceName));
}

export async function getSystemAudioSupport(): Promise<boolean> {
  return commands.getSystemAudioSupport();
}
