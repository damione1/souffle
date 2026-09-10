import { beforeEach, describe, expect, it, vi } from "vitest";

const {
  getSettings,
  saveSettings,
  getAppVersion,
  runStartupModelFlow,
  getMachineState,
  pillRelease,
  getDownloadProgress,
  getSystemAudioStatus,
  getModifierTapStatus,
} = vi.hoisted(() => ({
  getSettings: vi.fn(),
  saveSettings: vi.fn(),
  getAppVersion: vi.fn(),
  runStartupModelFlow: vi.fn(),
  getMachineState: vi.fn<() => Promise<AppStateMachine>>(async () => ({ state: "idle" })),
  pillRelease: vi.fn<() => Promise<void>>(async () => undefined),
  getDownloadProgress: vi.fn<() => Promise<DownloadProgress | null>>(async () => null),
  getSystemAudioStatus: vi.fn<() => Promise<SystemAudioStatus | null>>(async () => null),
  getModifierTapStatus: vi.fn<() => Promise<ModifierTapStatus | null>>(async () => null),
}));

vi.mock("./api/settings", () => ({
  getSettings,
  getSystemAudioStatus,
  getModifierTapStatus,
  saveSettings,
  selectAudioDevice: vi.fn(),
}));
vi.mock("./api/diagnostics", () => ({
  getAppVersion,
}));
vi.mock("./api/transcription", () => ({
  getDownloadProgress,
  getMachineState,
  pillRelease,
}));
// Keep the real `applyDownloadProgress`: the resync test below checks the
// counters it writes, not that it was called.
vi.mock("./features/transcription/runtime", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./features/transcription/runtime")>()),
  runStartupModelFlow,
}));
vi.mock("./utils/theme", () => ({
  applyTheme: vi.fn(),
}));

import { LOCAL_BUILD, bootstrapAppState } from "./bootstrap";
import { getAppState } from "./stores/app.svelte";
import { mockRuntimeStatus, mockSettings } from "./test-helpers/fixtures";
import { SETUP_STORAGE_KEY } from "./features/onboarding/setup";
import type { AppStateMachine, DownloadProgress, ModifierTapStatus, SystemAudioStatus } from "./types";

describe("bootstrapAppState what's new", () => {
  const app = getAppState();

  beforeEach(() => {
    localStorage.clear();
    app.settings = { ...mockSettings };
    app.showOnboarding = false;
    getSettings.mockResolvedValue({ ...mockSettings });
    saveSettings.mockResolvedValue(undefined);
    getAppVersion.mockResolvedValue("0.4.0");
    runStartupModelFlow.mockResolvedValue(undefined);
  });

  it("never shows the changelog on a first launch", async () => {
    const result = await bootstrapAppState(app);
    expect(result.whatsNew).toBeNull();
    expect(saveSettings).toHaveBeenCalledWith(
      expect.objectContaining({ last_seen_version: "0.4.0" }),
    );
  });

  it("never shows the changelog while setup is unfinished, even after a version bump", async () => {
    getSettings.mockResolvedValue({ ...mockSettings, last_seen_version: "0.3.0" });
    getAppVersion.mockResolvedValue("0.4.0");

    const result = await bootstrapAppState(app);
    expect(result.whatsNew).toBeNull();
    expect(saveSettings).toHaveBeenCalledWith(
      expect.objectContaining({ last_seen_version: "0.4.0" }),
    );
  });

  it("shows the changelog after setup when the version changed", async () => {
    localStorage.setItem(SETUP_STORAGE_KEY, "1");
    getSettings.mockResolvedValue({ ...mockSettings, last_seen_version: "0.3.0" });
    getAppVersion.mockResolvedValue("0.4.0");

    const result = await bootstrapAppState(app);
    expect(result.whatsNew).toEqual({
      version: "0.4.0",
      releaseNotes: "Updated to v0.4.0.",
    });
  });

  it("does not show the changelog when setup is done and the version is unchanged", async () => {
    localStorage.setItem(SETUP_STORAGE_KEY, "1");
    getSettings.mockResolvedValue({ ...mockSettings, last_seen_version: "0.4.0" });

    const result = await bootstrapAppState(app);
    expect(result.whatsNew).toBeNull();
  });

  it("shows no changelog for a local build, and leaves last_seen_version alone", async () => {
    localStorage.setItem(SETUP_STORAGE_KEY, "1");
    getSettings.mockResolvedValue({ ...mockSettings, last_seen_version: "0.10.0" });
    getAppVersion.mockResolvedValue(LOCAL_BUILD);

    const result = await bootstrapAppState(app);

    expect(result.whatsNew).toBeNull();
    // Stamping "local build" here would swallow the next real release's
    // changelog, since that release would then differ from what was stored.
    expect(saveSettings).not.toHaveBeenCalledWith(
      expect.objectContaining({ last_seen_version: LOCAL_BUILD }),
    );
  });
});

// A webview reload (crash of the render process, or ⌘R in dev) while the
// backend keeps running: the machine enum is synced, and so must be the three
// stores that used to restart from zero.
describe("bootstrapAppState webview reload resync (SOU-073)", () => {
  const app = getAppState();
  const profile = mockRuntimeStatus.profile;
  const downloading: AppStateMachine = { state: "downloading", data: { profile } };
  const meeting: AppStateMachine = {
    state: "recording_meeting",
    data: { profile, session_id: 1, meeting_id: "m1" },
  };

  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    app.settings = { ...mockSettings };
    app.machineState = { state: "idle" };
    app.systemAudioStatus = null;
    app.modifierTapStatus = null;
    app.downloadFile = "";
    app.downloadCompletedFiles = 0;
    app.downloadTotalFiles = 0;
    app.downloadedBytes = 0;
    app.downloadTotalBytes = null;
    getSettings.mockResolvedValue({ ...mockSettings });
    saveSettings.mockResolvedValue(undefined);
    getAppVersion.mockResolvedValue("0.4.0");
    runStartupModelFlow.mockResolvedValue(undefined);
  });

  it("releases a pill hold left over from before the reload when not recording", async () => {
    // Dictation polish held the pill, then the page died before its
    // pillRelease: the machine is Ready and `pill::sync` sees no rising edge.
    getMachineState.mockResolvedValueOnce({ state: "ready", data: { profile } });

    await bootstrapAppState(app);

    expect(pillRelease).toHaveBeenCalledTimes(1);
  });

  it("never releases the pill while a session is recording", async () => {
    getMachineState.mockResolvedValueOnce(meeting);
    await bootstrapAppState(app);

    getMachineState.mockResolvedValueOnce({
      state: "recording_dictation",
      data: { profile, session_id: 2 },
    });
    await bootstrapAppState(app);

    expect(pillRelease).not.toHaveBeenCalled();
  });

  it("restores the download gauge from the backend snapshot while a download is in flight", async () => {
    getMachineState.mockResolvedValueOnce(downloading);
    getDownloadProgress.mockResolvedValueOnce({
      file: "model.safetensors",
      downloaded_bytes: 300,
      total_bytes: 1000,
      completed_files: 1,
      total_files: 3,
      status: "downloading",
    });

    await bootstrapAppState(app);

    expect(app.downloadFile).toBe("model.safetensors");
    expect(app.downloadedBytes).toBe(300);
    expect(app.downloadTotalBytes).toBe(1000);
    expect(app.downloadCompletedFiles).toBe(1);
    expect(app.downloadTotalFiles).toBe(3);
  });

  it("does not read the download snapshot when nothing is downloading", async () => {
    getMachineState.mockResolvedValueOnce({ state: "ready", data: { profile } });

    await bootstrapAppState(app);

    expect(getDownloadProgress).not.toHaveBeenCalled();
    expect(app.downloadedBytes).toBe(0);
  });

  it("restores the system-audio badge for a meeting the previous webview started", async () => {
    getMachineState.mockResolvedValueOnce(meeting);
    getSystemAudioStatus.mockResolvedValueOnce({
      active: false,
      reason: "Screen Recording permission denied",
      reason_code: "permission_denied",
      samples: 0,
      signal_samples: 0,
    });

    await bootstrapAppState(app);

    expect(app.systemAudioStatus).toEqual({
      active: false,
      reason: "Screen Recording permission denied",
      reason_code: "permission_denied",
      samples: 0,
      signal_samples: 0,
    });
  });

  it("leaves the system-audio badge alone outside a meeting", async () => {
    getMachineState.mockResolvedValueOnce({ state: "ready", data: { profile } });

    await bootstrapAppState(app);

    expect(getSystemAudioStatus).not.toHaveBeenCalled();
    expect(app.systemAudioStatus).toBeNull();
  });

  it("restores native PTT tap status after a webview reload (SOU-116 AC7)", async () => {
    getMachineState.mockResolvedValueOnce({ state: "ready", data: { profile } });
    getModifierTapStatus.mockResolvedValueOnce({ installed: false });

    await bootstrapAppState(app);

    expect(getModifierTapStatus).toHaveBeenCalledTimes(1);
    expect(app.modifierTapStatus).toEqual({ installed: false });

    // Remount / second bootstrap must query again — status must not live
    // only in the previous webview's event listener.
    getMachineState.mockResolvedValueOnce({ state: "ready", data: { profile } });
    getModifierTapStatus.mockResolvedValueOnce({ installed: true });
    app.modifierTapStatus = null;

    await bootstrapAppState(app);

    expect(app.modifierTapStatus).toEqual({ installed: true });
  });

  it("does not let a stale tap snapshot overwrite a live install event (SOU-116)", async () => {
    let resolveStatus: (value: ModifierTapStatus | null) => void = () => {};
    getMachineState.mockResolvedValueOnce({ state: "ready", data: { profile } });
    getModifierTapStatus.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveStatus = resolve;
        }),
    );

    const boot = bootstrapAppState(app);
    await vi.waitFor(() => expect(getModifierTapStatus).toHaveBeenCalledTimes(1));
    app.modifierTapStatus = { installed: true };
    resolveStatus({ installed: false });
    await boot;

    expect(app.modifierTapStatus).toEqual({ installed: true });
  });

  it("keeps booting when a resync read fails", async () => {
    getMachineState.mockResolvedValueOnce(meeting);
    getSystemAudioStatus.mockRejectedValueOnce(new Error("backend busy"));
    getModifierTapStatus.mockRejectedValueOnce(new Error("backend busy"));

    await expect(bootstrapAppState(app)).resolves.toEqual({ whatsNew: null });
    expect(runStartupModelFlow).toHaveBeenCalledTimes(1);

    getMachineState.mockResolvedValueOnce({ state: "ready", data: { profile } });
    pillRelease.mockRejectedValueOnce(new Error("backend busy"));

    await expect(bootstrapAppState(app)).resolves.toEqual({ whatsNew: null });
  });
});
