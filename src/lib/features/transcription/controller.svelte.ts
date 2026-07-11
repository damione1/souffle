import { getAppState } from "../../stores/app.svelte";
import {
  addDictationEntry,
  getTranscriptionCatalog,
  pasteText,
  pillHold,
  pillRelease,
  startStreamingTranscription,
  stopStreamingTranscription,
} from "../../api/transcription";
import { events } from "../../api/generated";
import { createTimelineController } from "../timeline/controller.svelte";
import type { TranscriptionCatalog, TranscriptionSegment } from "../../types";
import { errorMessage } from "../../utils";
import { formatSelectedTranscriptionLabel } from "./catalog";
import { ensureModelLoaded, refreshTranscriptionRuntimeStatus } from "./runtime";

/**
 * Matches the accessibility error clipboard.rs returns when
 * `permissions::accessibility_granted()` fails at paste time (see
 * `ACCESSIBILITY_STALE_ERROR` in src-tauri/src/clipboard.rs). Distinct from
 * a raw Enigo error, so we can point the user at the repair action instead
 * of just relaying the OS string.
 */
function accessibilityPasteFailureMessage(rawMessage: string): string {
  if (rawMessage.includes("Accessibility permission missing")) {
    return "Paste failed: accessibility permission needed. Open Settings > Advanced > Permissions and use Repair permission.";
  }
  return `Paste failed: ${rawMessage}`;
}

/** Finalize dictation text: invisible-char strip, optional LLM polish, skip-if-blank. */
async function finalizeDictationText(rawText: string): Promise<{ text: string; warning?: string }> {
  const trimmed = rawText.trim();
  if (!trimmed) {
    return { text: "" };
  }

  const app = getAppState();
  if (!app.settings.dictation_polish_enabled) {
    return { text: trimmed };
  }

  try {
    const { polishDictation } = await import("../../api/dictation");
    const result = await polishDictation(trimmed);
    return {
      text: result.text.trim(),
      warning: result.warning ?? undefined,
    };
  } catch (e) {
    console.warn("Dictation polish failed:", e);
    return { text: trimmed, warning: errorMessage(e) };
  }
}

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

      // Polish keeps running after the recording state ends (it's an LLM
      // call over the finalized text); hold the pill open now, before the
      // state machine leaves the recording state, so it doesn't flash shut
      // and reopen. Always released below, even on error, so a failed
      // polish or paste never leaves a zombie pill.
      const holdForPolish = app.settings.dictation_polish_enabled;
      if (holdForPolish) {
        try {
          await pillHold("polishing");
        } catch (e) {
          console.warn("Pill hold failed:", e);
        }
      }

      try {
        await stopStreamingTranscription();

        const finalized = await finalizeDictationText(transcript);
        if (finalized.warning) {
          statusMessage = finalized.warning;
        }

        await saveToHistory(finalized.text);

        if (finalized.text) {
          if (fromShortcut && app.settings.auto_paste) {
            try {
              await pasteText(
                finalized.text,
                app.settings.paste_delay_ms,
                app.settings.paste_method,
              );
            } catch (e) {
              statusMessage = accessibilityPasteFailureMessage(errorMessage(e));
            }
          } else {
            try {
              await navigator.clipboard.writeText(finalized.text);
            } catch {
              // Clipboard API may fail silently in some contexts
            }
          }
        }
      } catch (e) {
        statusMessage = errorMessage(e);
      } finally {
        if (holdForPolish) {
          try {
            await pillRelease();
          } catch (e) {
            console.warn("Pill release failed:", e);
          }
        }
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
      void finalizeDictationText(transcript).then(({ text, warning }) => {
        if (warning) statusMessage = warning;
        if (text) {
          void saveToHistory(text);
          statusMessage = "Recording was interrupted — the partial transcript was saved to history.";
        } else {
          statusMessage = "Recording was interrupted.";
        }
      });
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
