export type SettingsAnchor =
  | "transcription.model"
  | "transcription.dictionary"
  | "transcription.polish"
  | "transcription.snippets"
  | "ai.provider"
  | "ai.templates"
  | "audio.mic"
  | "audio.sounds"
  | "audio.format"
  | "interface.language"
  | "interface.shortcuts"
  | "meetings.calendar"
  | "system.permissions"
  | "system.data"
  | "system.about";

import type { SettingsTab } from "./open";

export function tabForAnchor(anchor: SettingsAnchor): SettingsTab {
  return anchor.split(".")[0] as SettingsTab;
}
