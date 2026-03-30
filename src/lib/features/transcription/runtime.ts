import { getAppState } from "../../stores/app.svelte";
import {
  downloadModel,
  getModelStatus,
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
          void refreshTranscriptionRuntimeStatus(app, catalog).catch((error) => {
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
