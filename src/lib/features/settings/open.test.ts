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

  it("opens the permissions repair panel without requiring a settings tab", () => {
    const app = getAppState();
    app.permissionsPanelOpen = false;

    openPermissionsRepair();

    expect(app.permissionsPanelOpen).toBe(true);
  });
});
