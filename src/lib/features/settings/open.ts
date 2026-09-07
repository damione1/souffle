import { getAppState } from "../../stores/app.svelte";

export type SettingsTab = "transcription" | "ai" | "audio" | "interface" | "meetings" | "system";

/** Open Settings on a tab. One-shot: SettingsView consumes the tab and
 * forgets it, so the next ordinary open still lands on Transcription. */
export function openSettings(options: { tab: SettingsTab }) {
  const app = getAppState();
  app.settingsInitialTab = options.tab;
  app.settingsOpen = true;
}

/** Open the permissions repair panel directly — not an itinerary through
 * Settings → System → Review. */
export function openPermissionsRepair() {
  const app = getAppState();
  app.permissionsPanelOpen = true;
}
