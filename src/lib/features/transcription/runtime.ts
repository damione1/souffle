import { getAppState } from "../../stores/app.svelte";
import {
  downloadModel,
  getModelStatus,
  getTranscriptionCatalog,
  loadModel,
} from "../../api/transcription";
import type {
  AppStateMachine,
  DownloadProgress,
  TranscriptionCatalog,
  TranscriptionRuntimePhase,
} from "../../types";
import { errorMessage } from "../../utils";
import { toSelectedTranscriptionProfileSelection } from "./catalog";
import {
  decideShowSetupWizard,
  markSetupComplete,
  readSetupFlags,
  shouldMigrateSetupComplete,
} from "../onboarding/setup";

type AppState = ReturnType<typeof getAppState>;

export function resetTranscriptionRuntimeState(app: AppState) {
  app.transcriptionRuntimePhase = "download_required";
  app.downloadFile = "";
  app.downloadCompletedFiles = 0;
  app.downloadTotalFiles = 0;
  app.downloadedBytes = 0;
  app.downloadTotalBytes = null;
}

export function currentTranscriptionSelection(
  app: AppState,
  catalog: TranscriptionCatalog | null,
) {
  return toSelectedTranscriptionProfileSelection(
    catalog,
    app.settings.transcription_engine_id,
    app.settings.transcription_model_id,
    app.settings.transcription_backend_id,
  );
}

/** Refresh the phase of the *selected* profile and return it, so callers can
 * act on the fresh value instead of re-reading state a later event may have
 * moved on. */
export async function refreshTranscriptionRuntimeStatus(
  app: AppState,
  catalog: TranscriptionCatalog | null,
): Promise<TranscriptionRuntimePhase> {
  const status = await getModelStatus(currentTranscriptionSelection(app, catalog));
  app.transcriptionRuntimePhase = status.phase;
  app.settings = {
    ...app.settings,
    transcription_engine_id: status.profile.engine_id,
    transcription_model_id: status.profile.model_id,
    transcription_backend_id: status.profile.backend_id ?? app.settings.transcription_backend_id,
  };
  return status.phase;
}

/** Whether a download's progress Channel is alive in this webview. A reload
 * drops it with the page, and the download thread then finishes into a dead
 * callback: no autoLoad, no counter reset (SOU-073). */
let downloadChannelLive = false;

/** Mirror one progress report into the download counters. Shared by the live
 * Channel callback and by the bootstrap resync after a webview reload. */
export function applyDownloadProgress(app: AppState, progress: DownloadProgress): void {
  app.downloadFile = progress.file;
  app.downloadCompletedFiles = progress.completed_files;
  app.downloadTotalFiles = progress.total_files;
  app.downloadedBytes = progress.downloaded_bytes;
  if (progress.total_bytes !== null) {
    app.downloadTotalBytes = progress.total_bytes;
  }
}

export async function startTranscriptionModelDownload(
  app: AppState,
  catalog: TranscriptionCatalog | null,
  setStatusMessage: (message: string) => void,
  options: { autoLoad?: boolean } = {},
) {
  if (app.transcriptionModelOperationState !== "idle") return;

  app.downloadFile = "";
  app.downloadCompletedFiles = 0;
  app.downloadTotalFiles = 0;
  app.downloadedBytes = 0;
  app.downloadTotalBytes = null;
  setStatusMessage("");

  downloadChannelLive = true;
  try {
    await downloadModel(
      currentTranscriptionSelection(app, catalog),
      (progress: DownloadProgress) => {
        applyDownloadProgress(app, progress);

        if (typeof progress.status === "object" && "error" in progress.status) {
          downloadChannelLive = false;
          setStatusMessage(`Download error: ${progress.status.error}`);
          return;
        }

        if (progress.status === "complete" && progress.file === "all") {
          downloadChannelLive = false;
          app.downloadFile = "";
          app.downloadedBytes = 0;
          app.downloadTotalBytes = null;
          void refreshTranscriptionRuntimeStatus(app, catalog)
            .then(() => {
              if (options.autoLoad && app.transcriptionRuntimePhase === "load_required") {
                return startTranscriptionModelLoad(app, catalog, setStatusMessage);
              }
            })
            .catch((error) => {
              setStatusMessage(errorMessage(error));
            });
          return;
        }

        if (progress.status === "complete") {
          app.downloadFile = `${progress.file} done`;
        }
      },
    );
  } catch (error) {
    downloadChannelLive = false;
    setStatusMessage(errorMessage(error));
  }
}

/** Whether a `StateChanged` marks a download finishing into a dead Channel.
 * Only the downloading → downloaded edge counts: an idle-timeout unload also
 * lands on `downloaded` (via `unloading`) and must not reload what it just
 * freed, and a repeated event has no edge to fire on. */
export function shouldLoadAfterOrphanedDownload(
  previous: AppStateMachine["state"],
  next: AppStateMachine["state"],
  phase: TranscriptionRuntimePhase,
  channelLive: boolean,
): boolean {
  return (
    previous === "downloading"
    && next === "downloaded"
    && phase === "load_required"
    && !channelLive
  );
}

/** Safety net for the autoLoad a webview reload lost with its progress
 * Channel (SOU-073). Called from the global StateChanged listener with the
 * machine state before and after the event. Loads at most once per
 * download; if `runStartupModelFlow` raced it, `load_model` reuses the
 * engine already loaded and the silent status callback shows no error. */
export function loadAfterOrphanedDownload(
  app: AppState,
  previous: AppStateMachine,
  next: AppStateMachine,
): void {
  if (
    !shouldLoadAfterOrphanedDownload(
      previous.state,
      next.state,
      app.transcriptionRuntimePhase,
      downloadChannelLive,
    )
  ) {
    return;
  }
  void startTranscriptionModelLoad(app, null, () => {});
}

/// What to do with the selected model when the app starts.
export function decideStartupModelAction(
  phase: AppState["transcriptionRuntimePhase"],
  machineState: string,
): "load" | "onboarding" | "none" {
  if (phase === "download_required") return "onboarding";
  // Only auto-load from a settled cold state; a webview reload while the
  // backend is loading/ready/recording must not re-trigger anything.
  if (phase === "load_required" && (machineState === "idle" || machineState === "downloaded")) {
    return "load";
  }
  return "none";
}

function applySetupWizardVisibility(app: AppState): void {
  const flags = readSetupFlags();
  if (shouldMigrateSetupComplete(app.transcriptionRuntimePhase, flags)) {
    markSetupComplete();
  }
  app.showOnboarding = decideShowSetupWizard(
    app.transcriptionRuntimePhase,
    readSetupFlags(),
    app.machineState.state,
  );
}

/** Startup flow: auto-load the last-selected model, or surface onboarding
 * when nothing is downloaded yet. Fire-and-forget from bootstrap; progress
 * reaches the UI through StateChanged events. */
export async function runStartupModelFlow(app: AppState): Promise<void> {
  let catalog: TranscriptionCatalog | null = null;
  try {
    catalog = await getTranscriptionCatalog();
  } catch {
    applySetupWizardVisibility(app);
    return; // Backend unavailable; StateChanged events will catch us up.
  }
  app.settings = {
    ...app.settings,
    transcription_engine_id: catalog.selected_engine_id,
    transcription_model_id: catalog.selected_model_id,
    transcription_backend_id: catalog.selected_backend_id,
  };

  try {
    await refreshTranscriptionRuntimeStatus(app, catalog);
  } catch {
    applySetupWizardVisibility(app);
    return;
  }

  switch (decideStartupModelAction(app.transcriptionRuntimePhase, app.machineState.state)) {
    case "load":
      void startTranscriptionModelLoad(app, catalog, () => {});
      break;
    case "onboarding":
    case "none":
      break;
  }

  applySetupWizardVisibility(app);
}

// Indirection so TypeScript re-reads the getter after the `await` below
// instead of reusing the "load_required" narrowing from the earlier check.
function currentPhase(app: AppState): AppState["transcriptionRuntimePhase"] {
  return app.transcriptionRuntimePhase;
}

/** Load the model on demand if it's sitting in "load_required" (e.g. after
 * an idle-timeout unload), then report whether it's ready to record. A
 * no-op success when already ready, and a no-op failure when nothing is
 * downloaded yet (the caller should send the user to onboarding instead). */
export async function ensureModelLoaded(
  app: AppState,
  catalog: TranscriptionCatalog | null,
  setStatusMessage: (message: string) => void,
): Promise<boolean> {
  if (currentPhase(app) === "ready") return true;
  if (currentPhase(app) !== "load_required") return false;
  await startTranscriptionModelLoad(app, catalog, setStatusMessage);
  return currentPhase(app) === "ready";
}

export async function startTranscriptionModelLoad(
  app: AppState,
  catalog: TranscriptionCatalog | null,
  setStatusMessage: (message: string) => void,
) {
  if (app.transcriptionModelOperationState !== "idle") return;
  setStatusMessage("");

  try {
    await loadModel(currentTranscriptionSelection(app, catalog));
    await refreshTranscriptionRuntimeStatus(app, catalog);
  } catch (error) {
    setStatusMessage(errorMessage(error));
  }
}
