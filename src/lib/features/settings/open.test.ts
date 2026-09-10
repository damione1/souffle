import { describe, expect, it } from "vitest";
import { getAppState } from "../../stores/app.svelte";
import { openPermissionsRepair, openSettings } from "./open";

describe("openSettings", () => {
  it("opens settings on the requested tab", () => {
    const app = getAppState();
    app.settingsOpen = false;
    app.settingsInitialAnchor = null;

    openSettings({ anchor: "transcription.model" });

    expect(app.settingsOpen).toBe(true);
    expect(app.settingsInitialAnchor).toBe("transcription.model");
  });

  /** An alert rendered inside Settings (the dictation-polish banner points at
   * the provider row on the same tab) still has to reach SettingsView, which
   * watches the target rather than reading it once at mount. */
  it("publishes a new target even when settings is already open", () => {
    const app = getAppState();
    app.settingsOpen = true;
    app.settingsInitialAnchor = null;

    openSettings({ anchor: "ai.provider" });

    expect(app.settingsOpen).toBe(true);
    expect(app.settingsInitialAnchor).toBe("ai.provider");
  });

  it("opens the permissions repair panel without requiring a settings tab", () => {
    const app = getAppState();
    app.permissionsPanelOpen = false;

    openPermissionsRepair();

    expect(app.permissionsPanelOpen).toBe(true);
  });
});
