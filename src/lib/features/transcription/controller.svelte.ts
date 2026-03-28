import { getAppState } from "../../stores/app.svelte";
import {
  addDictationEntry,
  clearDictationHistory,
  deleteDictationEntry,
  downloadModel,
  getModelStatus,
  getTranscriptionCatalog,
  listDictationEntries,
  loadModel,
  pasteText,
  startStreamingTranscription,
  stopStreamingTranscription,
} from "../../api/transcription";
import { events } from "../../api/generated";
import { saveSettings } from "../../api/settings";
import type {
  AppSettings,
  DictationEntry,
  DownloadProgress,
  TranscriptionCatalog,
  TranscriptionSegment,
} from "../../types";
import { errorMessage } from "../../utils";
import {
  formatSelectedTranscriptionLabel,
  getFirstAvailableTranscriptionBackend,
  getFirstAvailableTranscriptionModel,
  getSelectedTranscriptionBackend,
  toSelectedTranscriptionProfileSelection,
} from "./catalog";

export function createTranscriptionController() {
  const app = getAppState();

  let isStartingRecording = $state(false);
  let isStopping = $state(false);
  let transcript = $state("");
  let statusMessage = $state("");
  let catalog = $state<TranscriptionCatalog | null>(null);

  let modelDownloaded = $state(false);
  let modelLoaded = $state(false);
  let isDownloading = $state(false);
  let downloadFile = $state("");
  let isLoadingModel = $state(false);

  let history = $state<DictationEntry[]>([]);
  let expandedEntryId = $state<string | null>(null);

  let activeProfileLabel = $derived.by(() => {
    if (!catalog) return "Transcription model";
    return formatSelectedTranscriptionLabel(
      catalog,
      app.settings.transcription_engine_id,
      app.settings.transcription_model_id,
      app.settings.transcription_backend_id,
    ) || "Transcription model";
  });

  function currentSelection() {
    return toSelectedTranscriptionProfileSelection(
      catalog,
      app.settings.transcription_engine_id,
      app.settings.transcription_model_id,
      app.settings.transcription_backend_id,
    );
  }

  async function mount() {
    await Promise.all([refreshCatalog(), refreshRuntimeStatus(), loadHistory()]);

    const unlisten = await Promise.all([
      events.shortcutToggle.listen(() => {
        if (!isStartingRecording && !isStopping) void toggleRecording(true);
      }),
      events.shortcutPttStart.listen(() => {
        if (!app.isRecording && !isStartingRecording && !isStopping) void toggleRecording(true);
      }),
      events.shortcutPttStop.listen(() => {
        if (app.isRecording && !isStopping) void toggleRecording(true);
      }),
    ]);

    return () => {
      unlisten.forEach((fn) => fn());
    };
  }

  async function refreshCatalog() {
    try {
      catalog = await getTranscriptionCatalog();
      app.settings = {
        ...app.settings,
        transcription_engine_id: catalog.selected_engine_id,
        transcription_model_id: catalog.selected_model_id,
        transcription_backend_id: catalog.selected_backend_id,
      };
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  async function refreshRuntimeStatus() {
    try {
      const status = await getModelStatus(currentSelection());
      modelDownloaded = status.downloaded;
      modelLoaded = status.loaded;
      app.settings = {
        ...app.settings,
        transcription_engine_id: status.profile.engine_id,
        transcription_model_id: status.profile.model_id,
        transcription_backend_id: status.profile.backend_id ?? app.settings.transcription_backend_id,
      };
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  async function persistSelection(engineId: string, modelId: string, backendId: string) {
    const nextSettings: AppSettings = {
      ...app.settings,
      audio_device: app.selectedDevice || null,
      transcription_engine_id: engineId,
      transcription_model_id: modelId,
      transcription_backend_id: backendId,
    };

    await saveSettings(nextSettings);
    app.settings = nextSettings;
    await Promise.all([refreshCatalog(), refreshRuntimeStatus()]);
  }

  async function selectEngine(engineId: string) {
    if (!catalog) return;
    const engine = catalog.engines.find((candidate) => candidate.id === engineId);
    const fallbackModel = getFirstAvailableTranscriptionModel(engine ?? null);
    const fallbackBackendId = getFirstAvailableTranscriptionBackend(fallbackModel)?.id;
    if (!fallbackModel || !fallbackBackendId) return;
    await persistSelection(engineId, fallbackModel.id, fallbackBackendId);
  }

  async function selectModel(modelId: string) {
    const backend = getSelectedTranscriptionBackend(
      catalog,
      app.settings.transcription_engine_id,
      modelId,
      app.settings.transcription_backend_id,
    );
    await persistSelection(
      app.settings.transcription_engine_id,
      modelId,
      backend?.id ?? app.settings.transcription_backend_id,
    );
  }

  async function selectBackend(backendId: string) {
    await persistSelection(
      app.settings.transcription_engine_id,
      app.settings.transcription_model_id,
      backendId,
    );
  }

  async function loadHistory() {
    try {
      history = await listDictationEntries(50);
    } catch {
      history = [];
    }
  }

  async function addHistoryEntry(text: string) {
    if (!text.trim()) return;
    try {
      await addDictationEntry(text.trim());
      await loadHistory();
    } catch (e) {
      console.warn("Failed to save dictation entry:", e);
    }
  }

  async function removeHistoryEntry(id: string) {
    try {
      await deleteDictationEntry(id);
      if (expandedEntryId === id) expandedEntryId = null;
      await loadHistory();
    } catch (e) {
      console.warn("Failed to delete dictation entry:", e);
    }
  }

  async function resetHistory() {
    try {
      await clearDictationHistory();
      history = [];
      expandedEntryId = null;
    } catch (e) {
      console.warn("Failed to clear history:", e);
    }
  }

  async function handleDownloadModel() {
    isDownloading = true;
    statusMessage = "";
    downloadFile = "";

    try {
      await downloadModel(currentSelection(), (progress: DownloadProgress) => {
        downloadFile = progress.file;
        if (typeof progress.status === "object" && "error" in progress.status) {
          statusMessage = `Download error: ${progress.status.error}`;
          isDownloading = false;
        } else if (progress.status === "complete" && progress.file === "all") {
          isDownloading = false;
          modelDownloaded = true;
          downloadFile = "";
        } else if (progress.status === "complete") {
          downloadFile = `${progress.file} done`;
        }
      });
      await refreshRuntimeStatus();
    } catch (e) {
      statusMessage = errorMessage(e);
      isDownloading = false;
    }
  }

  async function handleLoadModel() {
    isLoadingModel = true;
    statusMessage = "";
    try {
      await loadModel(currentSelection());
      modelLoaded = true;
      await refreshRuntimeStatus();
    } catch (e) {
      statusMessage = errorMessage(e);
    } finally {
      isLoadingModel = false;
    }
  }

  async function toggleRecording(fromShortcut = false) {
    if (isStartingRecording || isStopping) return;

    if (app.isRecording) {
      isStopping = true;
      try {
        await stopStreamingTranscription();
        app.isRecording = false;
        app.recordingMode = "idle";

        await addHistoryEntry(transcript);

        if (transcript.trim()) {
          if (fromShortcut && app.settings.auto_paste) {
            try {
              await pasteText(transcript.trim(), app.settings.paste_delay_ms);
            } catch (e) {
              statusMessage = `Paste failed: ${errorMessage(e)}`;
            }
          } else {
            try {
              await navigator.clipboard.writeText(transcript.trim());
            } catch {
              // Clipboard API may fail silently in some contexts
            }
          }
        }
      } catch (e) {
        statusMessage = errorMessage(e);
      } finally {
        isStopping = false;
      }
      return;
    }

    if (!modelLoaded) {
      statusMessage = modelDownloaded
        ? "Load the selected transcription model before starting dictation."
        : "Download and load the selected transcription model before starting dictation.";
      return;
    }

    transcript = "";
    statusMessage = "";
    isStartingRecording = true;

    try {
      let lastTime = 0;
      const pauseThreshold = 1.5;

      await startStreamingTranscription((segment: TranscriptionSegment) => {
        if (segment.is_final) {
          if (transcript) {
            const gap = segment.start_time - lastTime;
            const endsWithSentence = /[.!?…]\s*$/.test(transcript);
            if (gap >= pauseThreshold && endsWithSentence && !transcript.endsWith("\n")) {
              transcript += "\n\n";
            } else if (!transcript.endsWith(" ") && !transcript.endsWith("\n") && !segment.text.startsWith(" ")) {
              transcript += " ";
            }
          }
          transcript += segment.text;
          lastTime = segment.start_time;
        }
      });
      app.isRecording = true;
      app.recordingMode = "dictation";
    } catch (e) {
      statusMessage = errorMessage(e);
    } finally {
      isStartingRecording = false;
    }
  }

  return {
    get app() { return app; },
    get isStartingRecording() { return isStartingRecording; },
    get isStopping() { return isStopping; },
    get transcript() { return transcript; },
    get statusMessage() { return statusMessage; },
    get catalog() { return catalog; },
    get modelDownloaded() { return modelDownloaded; },
    get modelLoaded() { return modelLoaded; },
    get isDownloading() { return isDownloading; },
    get downloadFile() { return downloadFile; },
    get isLoadingModel() { return isLoadingModel; },
    get history() { return history; },
    get expandedEntryId() { return expandedEntryId; },
    set expandedEntryId(id: string | null) { expandedEntryId = id; },
    get activeProfileLabel() { return activeProfileLabel; },
    mount,
    refreshCatalog,
    refreshRuntimeStatus,
    selectEngine,
    selectModel,
    selectBackend,
    handleDownloadModel,
    handleLoadModel,
    toggleRecording,
    removeHistoryEntry,
    resetHistory,
  };
}
