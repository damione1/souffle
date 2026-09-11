import { commands, unwrap } from "./generated";
import type {
  AppSettings,
  AudioInputDevice,
  ModifierTapStatus,
  ShortcutSettings,
  SystemAudioStatus,
} from "../types";

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

export async function listAudioDevices(): Promise<AudioInputDevice[]> {
  return unwrap(commands.listAudioDevices());
}

export async function selectAudioDevice(deviceUid: string): Promise<void> {
  await unwrap(commands.selectAudioDevice(deviceUid));
}

export async function getInputSampleRate(deviceUid: string): Promise<number> {
  return unwrap(commands.getInputSampleRate(deviceUid));
}

export async function resetInputSampleRate(deviceUid: string): Promise<number> {
  return unwrap(commands.resetInputSampleRate(deviceUid));
}

export async function getSystemAudioSupport(): Promise<boolean> {
  return commands.getSystemAudioSupport();
}

/** Last system-audio leg status of the current meeting. The event only fires
 * when the tap is (re)built, so a webview reloaded mid-meeting reads this
 * snapshot instead (SOU-073). `null` outside a meeting session. */
export async function getSystemAudioStatus(): Promise<SystemAudioStatus | null> {
  return commands.getSystemAudioStatus();
}

/** Last native PTT CGEventTap install status. The event only fires on
 * success/failure, so a reloaded webview reads this snapshot (SOU-116).
 * `null` before the first install attempt. */
export async function getModifierTapStatus(): Promise<ModifierTapStatus | null> {
  return commands.getModifierTapStatus();
}

export async function isLaptop(): Promise<boolean> {
  return commands.isLaptop();
}
