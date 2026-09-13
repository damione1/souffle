import {
  getSettings,
  getModifierTapStatus,
  getNativeShortcuts,
  getSystemAudioStatus,
  saveSettings,
  selectAudioDevice,
} from "./api/settings";
import { getAppVersion } from "./api/diagnostics";
import { getDownloadProgress, getMachineState, pillRelease } from "./api/transcription";
import { applyDownloadProgress, runStartupModelFlow } from "./features/transcription/runtime";
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

  await resyncAfterReload(app);

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

/** The truths a webview reload loses with the page while the backend keeps
 * running (SOU-073): the pill HOLD whose release call lived in the destroyed
 * dictation controller, the download gauge whose Channel died, and the
 * system-audio badge whose event already fired. Each is a read, never a
 * rebuild, and each failure is swallowed like the machine sync above. */
async function resyncAfterReload(app: ReturnType<typeof getAppState>): Promise<void> {
  const state = app.machineState.state;

  // A hold left by an interrupted polish has nobody left to release it, and
  // `pill::sync` only drops it on the next idle → recording edge. Never
  // release during a session: that would hide a live meeting's pill.
  if (state !== "recording_dictation" && state !== "recording_meeting") {
    try {
      await pillRelease();
    } catch {
      // Nothing held, or backend not ready: the pill is not showing anyway.
    }
  }

  if (state === "downloading") {
    try {
      const progress = await getDownloadProgress();
      if (progress) applyDownloadProgress(app, progress);
    } catch {
      // Gauge stays at zero; the download itself is unaffected.
    }
  }

  if (state === "recording_meeting") {
    try {
      app.systemAudioStatus = await getSystemAudioStatus();
    } catch {
      // Badge stays hidden until the next tap rebuild emits.
    }
  }

  // Native PTT tap status is edge-triggered; restore after webview reload
  // so the settings banner can reappear without waiting for the next retry
  // (SOU-116 / AC7). A live event that arrived while this read was in
  // flight is newer: the retry loop stops on success, so overwriting
  // `{ installed: true }` with a stale `{ installed: false }` would stick
  // the Accessibility banner until the next reload.
  try {
    const snapshot = await getModifierTapStatus();
    if (app.modifierTapStatus === null) {
      app.modifierTapStatus = snapshot;
    }
  } catch {
    // Banner stays hidden until the next install attempt emits.
  }

  // The tap's key list, so the settings UI can flag a native binding without
  // restating the seventeen values (SOU-139). Left empty on failure: the
  // banner then stays hidden, which is what it already does before the first
  // tap status arrives.
  try {
    app.nativeShortcuts = await getNativeShortcuts();
  } catch {
    // Keep the empty list.
  }
}

export function whatsNewFallback(version: string): string {
  return `Updated to v${version}.`;
}
