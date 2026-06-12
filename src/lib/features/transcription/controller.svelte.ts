import { getAppState } from "../../stores/app.svelte";
import {
  addDictationEntry,
  clearDictationHistory,
  deleteDictationEntry,
  deleteModel,
  getTranscriptionCatalog,
  listDictationEntries,
  pasteText,
  startStreamingTranscription,
  stopStreamingTranscription,
} from "../../api/transcription";
import { events } from "../../api/generated";
import { saveSettings } from "../../api/settings";
import type {
  AppSettings,
  DictationEntry,
  TranscriptionCatalog,
  TranscriptionSegment,
} from "../../types";
import { createDebouncedSearch, errorMessage, matchedIdsForType } from "../../utils";
import {
  formatSelectedTranscriptionLabel,
  getFirstAvailableTranscriptionBackend,
  getFirstAvailableTranscriptionModel,
  getSelectedTranscriptionBackend,
} from "./catalog";
import {
  currentTranscriptionSelection,
  refreshTranscriptionRuntimeStatus,
  resetTranscriptionRuntimeState,
  startTranscriptionModelDownload,
  startTranscriptionModelLoad,
} from "./runtime";

function createTranscriptionControllerInstance() {
  const app = getAppState();

  let isStartingRecording = $state(false);
  let isStopping = $state(false);
  let transcript = $state("");
  let statusMessage = $state("");
  let catalog = $state<TranscriptionCatalog | null>(null);

  // Incremented for every session start (and on abort) so segment-channel
  // callbacks from a previous session can never write into a new one.
  let sessionGeneration = 0;

  let history = $state<DictationEntry[]>([]);
  let expandedEntryId = $state<string | null>(null);
  let historySearchQuery = $state("");
  const historySearch = createDebouncedSearch(250, 50);

  let filteredHistory = $derived.by(() => {
    const query = historySearchQuery.trim().toLowerCase();
    if (!query) return history;

    if (historySearch.results.length > 0) {
      const matched = matchedIdsForType(historySearch.results, "dictation");
      return history.filter((entry) => matched.has(entry.id));
    }

    // Fallback to local text filtering while search is in progress
    return history.filter((entry) => entry.text.toLowerCase().includes(query));
  });

  let activeProfileLabel = $derived.by(() => {
    if (!catalog) return "Transcription model";
    return formatSelectedTranscriptionLabel(
      catalog,
      app.settings.transcription_engine_id,
      app.settings.transcription_model_id,
      app.settings.transcription_backend_id,
    ) || "Transcription model";
  });

  async function mount() {
    await refreshCatalog();
    await Promise.all([refreshRuntimeStatus(), loadHistory()]);

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
      await refreshTranscriptionRuntimeStatus(app, catalog);
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
    resetTranscriptionRuntimeState(app);
    await refreshCatalog();
    await refreshRuntimeStatus();
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

  function onHistorySearchQueryChange(query: string) {
    historySearchQuery = query;
    historySearch.update(query);
  }

  async function handleDownloadModel() {
    await startTranscriptionModelDownload(app, catalog, (message) => {
      statusMessage = message;
    });
  }

  async function handleLoadModel() {
    await startTranscriptionModelLoad(app, catalog, (message) => {
      statusMessage = message;
    });
  }

  async function handleDeleteModel() {
    try {
      const selection = currentTranscriptionSelection(app, catalog);
      await deleteModel(selection);
      resetTranscriptionRuntimeState(app);
      await refreshRuntimeStatus();
    } catch (error) {
      statusMessage = errorMessage(error);
    }
  }

  async function toggleRecording(fromShortcut = false) {
    if (isStartingRecording || isStopping) return;

    if (app.isRecording) {
      isStopping = true;
      try {
        await stopStreamingTranscription();

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

    if (app.transcriptionRuntimePhase !== "ready") {
      statusMessage = app.transcriptionRuntimePhase === "download_required"
        ? "Download and load the model before starting dictation."
        : "Load the model before starting dictation.";
      return;
    }

    transcript = "";
    statusMessage = "";
    isStartingRecording = true;
    sessionGeneration += 1;
    const generation = sessionGeneration;

    try {
      await startStreamingTranscription((segment: TranscriptionSegment) => {
        if (generation !== sessionGeneration) return; // stale session
        if (segment.is_final) {
          if (transcript) {
            if (!transcript.endsWith(" ") && !segment.text.startsWith(" ")) {
              transcript += " ";
            }
          }
          transcript += segment.text;
        }
      });
    } catch (e) {
      statusMessage = errorMessage(e);
    } finally {
      isStartingRecording = false;
    }
  }

  /** The backend aborted the recording session (machine went to Error). */
  function handleRecordingAborted() {
    sessionGeneration += 1; // cut off in-flight segments from the dead session
    isStartingRecording = false;
    isStopping = false;
    if (transcript.trim()) {
      void addHistoryEntry(transcript);
      statusMessage = "Recording was interrupted — the partial transcript was saved to history.";
    } else {
      statusMessage = "Recording was interrupted.";
    }
  }

  return {
    get app() { return app; },
    get isStartingRecording() { return isStartingRecording; },
    get isStopping() { return isStopping; },
    get transcript() { return transcript; },
    get statusMessage() { return statusMessage; },
    get catalog() { return catalog; },
    get runtimePhase() { return app.transcriptionRuntimePhase; },
    get modelOperationState() { return app.transcriptionModelOperationState; },
    get downloadFile() { return app.downloadFile; },
    get downloadCompletedFiles() { return app.downloadCompletedFiles; },
    get downloadTotalFiles() { return app.downloadTotalFiles; },
    get downloadedBytes() { return app.downloadedBytes; },
    get downloadTotalBytes() { return app.downloadTotalBytes; },
    get history() { return history; },
    get filteredHistory() { return filteredHistory; },
    get expandedEntryId() { return expandedEntryId; },
    set expandedEntryId(id: string | null) { expandedEntryId = id; },
    get historySearchQuery() { return historySearchQuery; },
    set historySearchQuery(value: string) { onHistorySearchQueryChange(value); },
    get historySearchResults() { return historySearch.results; },
    get isSearchingHistory() { return historySearch.isSearching; },
    get activeProfileLabel() { return activeProfileLabel; },
    mount,
    refreshCatalog,
    refreshRuntimeStatus,
    selectEngine,
    selectModel,
    selectBackend,
    handleDownloadModel,
    handleLoadModel,
    handleDeleteModel,
    toggleRecording,
    removeHistoryEntry,
    resetHistory,
    handleRecordingAborted,
  };
}

/** Called from the global StateChanged listener when a dictation session
 * is aborted by the backend. No-op if the controller was never created. */
export function notifyDictationAborted() {
  instance?.handleRecordingAborted();
}

// Singleton: survives view mount/unmount cycles so transcript and Channel
// callbacks are never lost when the user switches tabs during recording.
let instance: ReturnType<typeof createTranscriptionControllerInstance> | null = null;

export function createTranscriptionController() {
  if (!instance) {
    instance = createTranscriptionControllerInstance();
  }
  return instance;
}

/** Reset the singleton for testing. */
export function resetTranscriptionControllerForTest() {
  instance = null;
}
