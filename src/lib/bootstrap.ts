import { getSettings, saveSettings, selectAudioDevice } from "./api/settings";
import { checkForUpdates, getAppVersion } from "./api/diagnostics";
import { getMachineState } from "./api/transcription";
import { runStartupModelFlow } from "./features/transcription/runtime";
import { setLocale } from "./i18n";
import { getAppState } from "./stores/app.svelte";
import { applyTheme } from "./utils/theme";

export type BootstrapResult = {
  whatsNew: { version: string; releaseNotes: string } | null;
};

export async function bootstrapAppState(
  app: ReturnType<typeof getAppState>,
): Promise<BootstrapResult> {
  // Sync the backend state machine first: on a webview reload the backend
  // may be Ready/Recording/Error while the store defaults to idle.
  try {
    app.machineState = await getMachineState();
  } catch {
    // Backend not ready yet — StateChanged events will sync us.
  }

  const settings = await getSettings();
  app.settings = settings;
  app.selectedDevice = settings.audio_device ?? "";
  applyTheme(app.settings.theme);

  if (settings.locale) {
    setLocale(settings.locale);
  }

  if (settings.audio_device) {
    await selectAudioDevice(settings.audio_device);
  }

  // Zero-ceremony startup: auto-load the last-selected model, or show
  // first-run onboarding when no model is downloaded yet.
  await runStartupModelFlow(app);

  const currentVersion = await getAppVersion();
  const previousVersion = settings.last_seen_version.trim();

  if (!previousVersion) {
    // Brand-new install: record version silently, no What's New dialog.
    if (settings.last_seen_version !== currentVersion) {
      await saveSettings({ ...settings, last_seen_version: currentVersion });
      app.settings = { ...app.settings, last_seen_version: currentVersion };
    }
    return { whatsNew: null };
  }

  if (previousVersion === currentVersion) {
    return { whatsNew: null };
  }

  const update = await checkForUpdates();
  const releaseNotes =
    update.release_notes?.trim() ||
    (update.latest_version
      ? `Updated to v${currentVersion}.`
      : `Updated to v${currentVersion}.`);

  return {
    whatsNew: {
      version: currentVersion,
      releaseNotes,
    },
  };
}
