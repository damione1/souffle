import { getSettings, saveSettings, selectAudioDevice } from "./api/settings";
import { getAppVersion } from "./api/diagnostics";
import { getMachineState } from "./api/transcription";
import { runStartupModelFlow } from "./features/transcription/runtime";
import { readSetupFlags } from "./features/onboarding/setup";
import { setLocale } from "./i18n";
import { getAppState } from "./stores/app.svelte";
import { applyTheme } from "./utils/theme";

export type BootstrapResult = {
  whatsNew: { version: string; releaseNotes: string } | null;
};

/** Mirrors `update_check::LOCAL_BUILD`: what get_app_version returns from a
 * checkout the release workflow never stamped. */
export const LOCAL_BUILD = "local build";

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
  // the first-run setup wizard when the user hasn't finished onboarding
  // (or when no model is downloaded yet).
  await runStartupModelFlow(app);

  const currentVersion = await getAppVersion();
  const previousVersion = app.settings.last_seen_version.trim();
  const setupDone = readSetupFlags().setupDone;

  // A build made from a checkout has no release notes to show, and its version
  // string is not a number, so "Updated to vlocal build." would be the whole
  // dialog. Leave last_seen_version alone too: the next real release should
  // still announce itself.
  if (currentVersion === LOCAL_BUILD) {
    return { whatsNew: null };
  }

  // First launch and unfinished setup: stamp the version silently so the
  // changelog never stacks on the wizard, and so finishing setup doesn't
  // immediately pop it either.
  if (!previousVersion || !setupDone) {
    if (app.settings.last_seen_version !== currentVersion) {
      const next = { ...app.settings, last_seen_version: currentVersion };
      await saveSettings(next);
      app.settings = next;
    }
    return { whatsNew: null };
  }

  if (previousVersion === currentVersion) {
    return { whatsNew: null };
  }

  return {
    whatsNew: {
      version: currentVersion,
      releaseNotes: whatsNewFallback(currentVersion),
    },
  };
}

export function whatsNewFallback(version: string): string {
  return `Updated to v${version}.`;
}
