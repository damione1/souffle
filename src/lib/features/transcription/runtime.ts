import { getAppState } from "../../stores/app.svelte";
import {
  downloadModel,
  getModelStatus,
  getTranscriptionCatalog,
  loadModel,
} from "../../api/transcription";
import type { DownloadProgress, TranscriptionCatalog } from "../../types";
import { errorMessage } from "../../utils";
import { toSelectedTranscriptionProfileSelection } from "./catalog";

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

export async function refreshTranscriptionRuntimeStatus(
  app: AppState,
  catalog: TranscriptionCatalog | null,
) {
  const status = await getModelStatus(currentTranscriptionSelection(app, catalog));
  app.transcriptionRuntimePhase = status.phase;
  app.settings = {
    ...app.settings,
    transcription_engine_id: status.profile.engine_id,
    transcription_model_id: status.profile.model_id,
    transcription_backend_id: status.profile.backend_id ?? app.settings.transcription_backend_id,
  };
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

  try {
    await downloadModel(
      currentTranscriptionSelection(app, catalog),
      (progress: DownloadProgress) => {
        app.downloadFile = progress.file;
        app.downloadCompletedFiles = progress.completed_files;
        app.downloadTotalFiles = progress.total_files;
        app.downloadedBytes = progress.downloaded_bytes;
        if (progress.total_bytes !== null) {
          app.downloadTotalBytes = progress.total_bytes;
        }

        if (typeof progress.status === "object" && "error" in progress.status) {
          setStatusMessage(`Download error: ${progress.status.error}`);
          return;
        }

        if (progress.status === "complete" && progress.file === "all") {
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
    setStatusMessage(errorMessage(error));
  }
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

/** Startup flow: auto-load the last-selected model, or surface onboarding
 * when nothing is downloaded yet. Fire-and-forget from bootstrap; progress
 * reaches the UI through StateChanged events. */
export async function runStartupModelFlow(app: AppState): Promise<void> {
  let catalog: TranscriptionCatalog | null = null;
  try {
    catalog = await getTranscriptionCatalog();
  } catch {
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
    return;
  }

  switch (decideStartupModelAction(app.transcriptionRuntimePhase, app.machineState.state)) {
    case "onboarding":
      app.showOnboarding = true;
      break;
    case "load":
      void startTranscriptionModelLoad(app, catalog, () => {});
      break;
    case "none":
      break;
  }
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
