import { getSettings, selectAudioDevice } from "./api/settings";
import { getMachineState } from "./api/transcription";
import { runStartupModelFlow } from "./features/transcription/runtime";
import { setLocale } from "./i18n";
import { getAppState } from "./stores/app.svelte";
import { applyTheme } from "./utils/theme";

export async function bootstrapAppState(
  app: ReturnType<typeof getAppState>,
): Promise<void> {
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
}
