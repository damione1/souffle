import { beforeEach, describe, expect, it } from "vitest";
import {
  SETUP_STORAGE_KEY,
  PERMISSIONS_STORAGE_KEY,
  decideShowSetupWizard,
  markPermissionsDone,
  markSetupComplete,
  readSetupFlags,
  shouldMigrateSetupComplete,
  wizardSteps,
} from "./setup";

describe("setup flags", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("starts unset", () => {
    expect(readSetupFlags()).toEqual({ permissionsDone: false, setupDone: false });
  });

  it("markPermissionsDone is independent of setupDone", () => {
    markPermissionsDone();
    expect(readSetupFlags()).toEqual({ permissionsDone: true, setupDone: false });
  });

  it("markSetupComplete sets both flags", () => {
    markSetupComplete();
    expect(localStorage.getItem(PERMISSIONS_STORAGE_KEY)).toBe("1");
    expect(localStorage.getItem(SETUP_STORAGE_KEY)).toBe("1");
    expect(readSetupFlags()).toEqual({ permissionsDone: true, setupDone: true });
  });
});

describe("shouldMigrateSetupComplete", () => {
  it("migrates pre-wizard users who already finished permissions and have a model", () => {
    expect(
      shouldMigrateSetupComplete("ready", { permissionsDone: true, setupDone: false }),
    ).toBe(true);
  });

  it("does not migrate a first-run still missing a model", () => {
    expect(
      shouldMigrateSetupComplete("download_required", {
        permissionsDone: true,
        setupDone: false,
      }),
    ).toBe(false);
  });

  it("does not migrate when setup is already complete", () => {
    expect(
      shouldMigrateSetupComplete("ready", { permissionsDone: true, setupDone: true }),
    ).toBe(false);
  });
});

describe("decideShowSetupWizard", () => {
  it("shows the full wizard on a brand-new install", () => {
    expect(
      decideShowSetupWizard("download_required", {
        permissionsDone: false,
        setupDone: false,
      }),
    ).toBe(true);
  });

  it("keeps showing after the model is ready until the wizard is finished", () => {
    expect(
      decideShowSetupWizard("ready", { permissionsDone: false, setupDone: false }),
    ).toBe(true);
  });

  it("keeps showing after permissions if the model is still missing", () => {
    expect(
      decideShowSetupWizard("download_required", {
        permissionsDone: true,
        setupDone: false,
      }),
    ).toBe(true);
  });

  it("hides for migrated existing users", () => {
    expect(
      decideShowSetupWizard("ready", { permissionsDone: true, setupDone: true }),
    ).toBe(false);
  });

  it("reopens on a model-only recovery after the user deleted the files", () => {
    expect(
      decideShowSetupWizard("download_required", {
        permissionsDone: true,
        setupDone: true,
      }),
    ).toBe(true);
  });
});

describe("wizardSteps", () => {
  it("starts at permissions for a new user", () => {
    expect(wizardSteps({ permissionsDone: false, setupDone: false })).toEqual([
      "permissions",
      "microphone",
      "model",
      "shortcut",
    ]);
  });

  it("resumes at microphone after permissions were granted", () => {
    expect(wizardSteps({ permissionsDone: true, setupDone: false })).toEqual([
      "microphone",
      "model",
      "shortcut",
    ]);
  });

  it("is model-only when setup was already completed", () => {
    expect(wizardSteps({ permissionsDone: true, setupDone: true })).toEqual(["model"]);
  });
});
