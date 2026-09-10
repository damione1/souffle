import type { AppStateMachine, TranscriptionRuntimePhase } from "../../types";

export const PERMISSIONS_STORAGE_KEY = "permissionsOnboarded";
export const SETUP_STORAGE_KEY = "setupOnboarded";

export type SetupStep = "permissions" | "microphone" | "model" | "shortcut" | "interview";

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
  machineState: AppStateMachine["state"],
): boolean {
  if (flags.setupDone) {
    // Model-only recovery after the user deleted the files. Not while the
    // backend is still downloading: a webview reload mid-download reports
    // "download_required" too, and must not reopen the wizard over a
    // download that is about to finish (SOU-073).
    return phase === "download_required" && machineState !== "downloading";
  }
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

export function wizardSteps(flags: SetupFlags, dictionaryInterviewDone: boolean): SetupStep[] {
  if (flags.setupDone) return ["model"];
  const steps: SetupStep[] = [];
  if (!flags.permissionsDone) steps.push("permissions");
  steps.push("microphone", "model", "shortcut");
  if (!dictionaryInterviewDone) steps.push("interview");
  return steps;
}
