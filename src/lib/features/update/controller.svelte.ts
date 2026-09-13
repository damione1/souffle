import { onMount } from "svelte";
import {
  cancelUpdateDownload,
  downloadUpdate,
  getUpdateDownloadStatus,
  getUpdateInstallBlock,
  installUpdate,
  listenUpdateDownloadProgress,
  type InstallBlockReason,
  type UpdateDownloadStatus,
  type UpdatePhase,
} from "../../api/updater";
import { openReleasePage } from "../../api/diagnostics";
import { errorMessage } from "../../utils";
import { getAppState } from "../../stores/app.svelte";

/** Machine states that must disable Install (mirrors Rust `install_blocked_reason`). */
const BLOCKING_STATES = new Set([
  "recording_dictation",
  "recording_meeting",
  "stopping",
  "downloading",
  "loading",
  "unloading",
]);

function blockFromMachine(state: string): InstallBlockReason | null {
  if (!BLOCKING_STATES.has(state)) return null;
  return state as InstallBlockReason;
}

function idleStatus(): UpdateDownloadStatus {
  return {
    phase: "idle",
    version: null,
    downloaded_bytes: 0,
    total_bytes: null,
    error: null,
    manual_fallback: false,
  };
}

/** Shared mirror of the Rust updater download state. Survives dialog close. */
class UpdateController {
  status = $state<UpdateDownloadStatus>(idleStatus());
  busy = $state(false);
  actionError = $state<string | null>(null);
  installBlock = $state<InstallBlockReason | null>(null);
  #started = false;
  #unlisten: (() => void) | null = null;

  get phase(): UpdatePhase {
    return this.status.phase;
  }

  /** Keep installBlock in sync with the live machine (and a backend query). */
  syncInstallBlock() {
    const app = getAppState();
    const fromMachine = blockFromMachine(app.machineState.state);
    this.installBlock = fromMachine;
    void getUpdateInstallBlock()
      .then((block) => {
        this.installBlock = block ?? blockFromMachine(getAppState().machineState.state);
      })
      .catch(() => {
        this.installBlock = blockFromMachine(getAppState().machineState.state);
      });
  }

  async start() {
    if (this.#started) return;
    this.#started = true;
    try {
      this.status = await getUpdateDownloadStatus();
    } catch {
      this.status = idleStatus();
    }
    this.syncInstallBlock();
    this.#unlisten = await listenUpdateDownloadProgress((status) => {
      this.status = status;
    });
  }

  stop() {
    this.#unlisten?.();
    this.#unlisten = null;
    this.#started = false;
  }

  async download() {
    this.busy = true;
    this.actionError = null;
    try {
      this.status = await downloadUpdate();
    } catch (e) {
      this.actionError = errorMessage(e);
      this.status = {
        ...this.status,
        phase: "failed",
        error: this.actionError,
        manual_fallback: true,
      };
    } finally {
      this.busy = false;
    }
  }

  async cancel() {
    this.busy = true;
    this.actionError = null;
    try {
      this.status = await cancelUpdateDownload();
    } catch (e) {
      this.actionError = errorMessage(e);
    } finally {
      this.busy = false;
    }
  }

  async install() {
    this.syncInstallBlock();
    if (this.installBlock) return;
    this.busy = true;
    this.actionError = null;
    try {
      await installUpdate();
    } catch (e) {
      this.actionError = errorMessage(e);
      this.status = {
        ...this.status,
        phase: "failed",
        error: this.actionError,
        manual_fallback: true,
      };
    } finally {
      this.busy = false;
    }
  }

  openFallback(releaseUrl: string | null) {
    if (releaseUrl) void openReleasePage(releaseUrl);
  }
}

export const updateController = new UpdateController();

/** Ensure the controller is listening for the lifetime of a UI that needs it. */
export function useUpdateController() {
  onMount(() => {
    void updateController.start();
  });
}

// Re-sync install block whenever the machine changes (recording start/stop).
$effect.root(() => {
  $effect(() => {
    void getAppState().machineState.state;
    updateController.syncInstallBlock();
  });
});
