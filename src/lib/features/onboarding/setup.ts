import type { TranscriptionRuntimePhase } from "../../types";

export const PERMISSIONS_STORAGE_KEY = "permissionsOnboarded";
export const SETUP_STORAGE_KEY = "setupOnboarded";

export type SetupStep = "permissions" | "microphone" | "model" | "shortcut";

export type SetupFlags = {
  permissionsDone: boolean;
  setupDone: boolean;
};

function readFlag(key: string): boolean {
  try {
    return localStorage.getItem(key) === "1";
  } catch {
    return false;
  }
}

function writeFlag(key: string, value: boolean): void {
  try {
    if (value) localStorage.setItem(key, "1");
    else localStorage.removeItem(key);
  } catch {
    // Private mode / storage disabled — wizard just shows again next time.
  }
}

export function readSetupFlags(): SetupFlags {
  return {
    permissionsDone: readFlag(PERMISSIONS_STORAGE_KEY),
    setupDone: readFlag(SETUP_STORAGE_KEY),
  };
}

export function markPermissionsDone(): void {
  writeFlag(PERMISSIONS_STORAGE_KEY, true);
}

export function markSetupComplete(): void {
  writeFlag(PERMISSIONS_STORAGE_KEY, true);
  writeFlag(SETUP_STORAGE_KEY, true);
}

/** Pre-wizard installs already granted permissions and have a model on disk.
 * Treat them as fully set up so the new mic/shortcut steps don't reappear. */
export function shouldMigrateSetupComplete(
  phase: TranscriptionRuntimePhase,
  flags: SetupFlags,
): boolean {
  return !flags.setupDone && flags.permissionsDone && phase !== "download_required";
}

export function decideShowSetupWizard(
  phase: TranscriptionRuntimePhase,
  flags: SetupFlags,
): boolean {
  if (flags.setupDone) return phase === "download_required";
  if (flags.permissionsDone && phase !== "download_required") return false;
  return true;
}

/** SOU-036: what `autostart_enabled` should be once the wizard finishes.
 * A fresh install leaves the wizard with the login item registered; a
 * recovery run (model re-download on an already set-up install) keeps
 * whatever is stored, because an absent key on an existing install must not
 * turn into "on" without a gesture from the user. */
export function decideAutostartOnFinish(recoveryOnly: boolean, current: boolean): boolean {
  return recoveryOnly ? current : true;
}

/** First-run walks permissions (if needed) → mic → model → shortcut.
 * Re-download after a deleted model is model-only. */
export function wizardSteps(flags: SetupFlags): SetupStep[] {
  if (flags.setupDone) return ["model"];
  const steps: SetupStep[] = [];
  if (!flags.permissionsDone) steps.push("permissions");
  steps.push("microphone", "model", "shortcut");
  return steps;
}
