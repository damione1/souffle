import { beforeEach, describe, expect, it, vi } from "vitest";

const { getSettings, saveSettings, getAppVersion, runStartupModelFlow } = vi.hoisted(() => ({
  getSettings: vi.fn(),
  saveSettings: vi.fn(),
  getAppVersion: vi.fn(),
  runStartupModelFlow: vi.fn(),
}));

vi.mock("./api/settings", () => ({
  getSettings,
  saveSettings,
  selectAudioDevice: vi.fn(),
}));
vi.mock("./api/diagnostics", () => ({
  getAppVersion,
}));
vi.mock("./api/transcription", () => ({
  getMachineState: vi.fn().mockResolvedValue({ state: "idle" }),
}));
vi.mock("./features/transcription/runtime", () => ({
  runStartupModelFlow,
}));
vi.mock("./utils/theme", () => ({
  applyTheme: vi.fn(),
}));

import { LOCAL_BUILD, bootstrapAppState } from "./bootstrap";
import { getAppState } from "./stores/app.svelte";
import { mockSettings } from "./test-helpers/fixtures";
import { SETUP_STORAGE_KEY } from "./features/onboarding/setup";

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
