import { getSettings, selectAudioDevice, toAppSettings } from "./api/settings";
import { getAppState } from "./stores/app.svelte";
import { applyTheme } from "./utils/theme";

export async function bootstrapAppState(
  app: ReturnType<typeof getAppState>,
): Promise<void> {
  const settings = await getSettings();
  app.settings = toAppSettings(settings);
  applyTheme(app.settings.theme);

  if (settings.audio_device) {
    app.selectedDevice = settings.audio_device;
    await selectAudioDevice(settings.audio_device);
  }
}
