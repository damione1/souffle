import { getAppState } from "../../stores/app.svelte";
import {
  addDictationEntry,
  getTranscriptionCatalog,
  pasteText,
  startStreamingTranscription,
  stopStreamingTranscription,
} from "../../api/transcription";
import { events } from "../../api/generated";
import { createTimelineController } from "../timeline/controller.svelte";
import type { TranscriptionCatalog, TranscriptionSegment } from "../../types";
import { errorMessage } from "../../utils";
import { formatSelectedTranscriptionLabel } from "./catalog";
import { ensureModelLoaded, refreshTranscriptionRuntimeStatus } from "./runtime";

/** Persist a finished dictation and surface it in the timeline. */
async function saveToHistory(text: string) {
  if (!text.trim()) return;
  try {
    await addDictationEntry(text.trim());
    await createTimelineController().refresh();
  } catch (e) {
    console.warn("Failed to save dictation entry:", e);
  }
}

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
    await refreshRuntimeStatus();

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

  async function toggleRecording(fromShortcut = false) {
    if (isStartingRecording || isStopping) return;

    if (app.isRecording) {
      isStopping = true;
      try {
        await stopStreamingTranscription();

        await saveToHistory(transcript);

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

    if (app.transcriptionRuntimePhase === "download_required") {
      statusMessage = "Download and load the model before starting dictation.";
      return;
    }
    if (app.transcriptionRuntimePhase !== "ready") {
      // Model was unloaded (e.g. the idle timeout freed it); reload through
      // the normal load flow before recording instead of leaving the user
      // stuck with a disabled button.
      statusMessage = "";
      const ready = await ensureModelLoaded(app, catalog, (message) => { statusMessage = message; });
      if (!ready) {
        if (!statusMessage) statusMessage = "Load the model before starting dictation.";
        return;
      }
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
      void saveToHistory(transcript);
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
    get activeProfileLabel() { return activeProfileLabel; },
    mount,
    refreshCatalog,
    refreshRuntimeStatus,
    toggleRecording,
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
