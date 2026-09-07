import { getAppState } from "../../stores/app.svelte";
import {
  addDictationEntry,
  getTranscriptionCatalog,
  notifyPasteFailed,
  pasteText,
  pillHold,
  pillRelease,
  startStreamingTranscription,
  stopStreamingTranscription,
} from "../../api/transcription";
import { learnFromEdit } from "../../api/dictionary";
import { frontmostAppName, readFocusedText, readSelectedText } from "../../api/focus";
import { events } from "../../api/generated";
import { createTimelineController } from "../timeline/controller.svelte";
import type { TranscriptionCatalog, TranscriptionSegment } from "../../types";
import { errorMessage, segmentGap } from "../../utils";
import { formatSelectedTranscriptionLabel } from "./catalog";
import { ensureModelLoaded, refreshTranscriptionRuntimeStatus } from "./runtime";

const LEARN_FROM_EDIT_DELAY_MS = 4000;
const MAX_LEARN_FROM_EDIT_PAIRS = 8;

type SessionMode = "insert" | "rewrite";

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

function tokenizeWords(text: string): string[] {
  return text
    .split(/\s+/)
    .map((token) => token.replace(/^[^\p{L}\p{N}]+|[^\p{L}\p{N}]+$/gu, ""))
    .filter((token) => token.length > 0);
}

/** Word-level pair count aligned with `derive_corrections_from_edit`. */
function countCorrectionPairs(original: string, corrected: string): number {
  const origWords = tokenizeWords(original);
  const corrWords = tokenizeWords(corrected);
  if (origWords.length === 0 || corrWords.length === 0) return 0;

  let count = 0;
  let i = 0;
  let j = 0;
  const seen = new Set<string>();

  while (i < origWords.length && j < corrWords.length) {
    if (origWords[i].toLowerCase() === corrWords[j].toLowerCase()) {
      i += 1;
      j += 1;
      continue;
    }
    if (j + 1 < corrWords.length && origWords[i].toLowerCase() === corrWords[j + 1].toLowerCase()) {
      j += 1;
      continue;
    }
    if (i + 1 < origWords.length && origWords[i + 1].toLowerCase() === corrWords[j].toLowerCase()) {
      i += 1;
      continue;
    }
    const from = origWords[i];
    const to = corrWords[j];
    if (
      from.length >= 3
      && to.length >= 3
      && from.toLowerCase() !== to.toLowerCase()
    ) {
      const key = `${from.toLowerCase()}\0${to.toLowerCase()}`;
      if (!seen.has(key)) {
        seen.add(key);
        count += 1;
      }
    }
    i += 1;
    j += 1;
  }

  return count;
}

/** Finalize dictation text: invisible-char strip, optional LLM polish, skip-if-blank. */
async function finalizeDictationText(
  rawText: string,
  focusedApp: string | null,
  rewriteOf: string | null,
): Promise<{ text: string; warning?: string }> {
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
    const result = await polishDictation(trimmed, focusedApp, rewriteOf);
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
async function saveToHistory(text: string): Promise<boolean> {
  if (!text.trim()) return false;
  try {
    await addDictationEntry(text.trim());
    await createTimelineController().refresh();
    return true;
  } catch (e) {
    console.warn("Failed to save dictation entry:", e);
    return false;
  }
}

function createTranscriptionControllerInstance() {
  const app = getAppState();

  let isStartingRecording = $state(false);
  let isStopping = $state(false);
  let transcript = $state("");
  let tentative = $state("");
  let statusMessage = $state("");
  let catalog = $state<TranscriptionCatalog | null>(null);

  // Incremented for every session start (and on abort) so segment-channel
  // callbacks from a previous session can never write into a new one.
  let sessionGeneration = 0;
  let sessionMode: SessionMode = "insert";
  let focusedApp: string | null = null;
  let rewriteOf: string | null = null;
  let learnFromEditTimer: ReturnType<typeof setTimeout> | null = null;

  let activeProfileLabel = $derived.by(() => {
    if (!catalog) return "Transcription model";
    return formatSelectedTranscriptionLabel(
      catalog,
      app.settings.transcription_engine_id,
      app.settings.transcription_model_id,
      app.settings.transcription_backend_id,
    ) || "Transcription model";
  });

  // `app.isRecording` is true for a meeting too, but this controller only
  // owns dictation: it must never treat a meeting as "its" recording (same
  // derivation as the native HUD and meeting/controller.svelte.ts).
  let isDictating = $derived(app.recordingMode === "dictation");

  function cancelLearnFromEditPoll() {
    if (learnFromEditTimer != null) {
      clearTimeout(learnFromEditTimer);
      learnFromEditTimer = null;
    }
  }

  /**
   * Clears the current dictation session state.
   * This ensures that transient state (such as the target app to paste into
   * or the accumulated transcript) is reset, preventing state leakage between
   * independent dictation sessions or unrelated recording types.
   */
  function clearSessionContext() {
    focusedApp = null;
    rewriteOf = null;
    sessionMode = "insert";
    // Nothing keeps this alive once the session that produced it is over;
    // leaving it would let a later, unrelated stop re-paste and re-save it.
    transcript = "";
  }

  async function captureStartContext() {
    try {
      focusedApp = await frontmostAppName();
    } catch {
      focusedApp = null;
    }
    if (sessionMode === "rewrite") {
      try {
        rewriteOf = await readSelectedText();
      } catch {
        rewriteOf = null;
      }
    } else {
      rewriteOf = null;
    }
  }

  function scheduleLearnFromEdit(pasted: string) {
    cancelLearnFromEditPoll();
    if (!app.settings.dictation_learn_from_edit || !pasted) return;

    learnFromEditTimer = setTimeout(async () => {
      learnFromEditTimer = null;
      try {
        if (!app.settings.dictation_learn_from_edit) return;
        const focused = (await readFocusedText())?.trim() ?? null;
        if (!focused || focused === pasted) return;
        // Whole-field AX reads include pre-existing content around the paste.
        // Skip those so we don't learn eight unrelated word pairs.
        const pastedWords = tokenizeWords(pasted);
        const focusedWords = tokenizeWords(focused);
        if (focusedWords.length > pastedWords.length + 3) return;
        if (focused.length > pasted.length * 1.5 + 20) return;
        if (countCorrectionPairs(pasted, focused) > MAX_LEARN_FROM_EDIT_PAIRS) return;
        await learnFromEdit(pasted, focused);
      } catch {
        // Post-paste AX reads and dictionary writes are best-effort.
      }
    }, LEARN_FROM_EDIT_DELAY_MS);
  }

  async function mount() {
    await refreshCatalog();
    await refreshRuntimeStatus();

    const unlisten = await Promise.all([
      events.shortcutToggle.listen(() => {
        // A meeting owns the session: the dictation shortcut is a no-op,
        // not a way to stop someone else's recording.
        if (app.recordingMode === "meeting") return;
        if (!isStartingRecording && !isStopping) {
          if (!isDictating) sessionMode = "insert";
          void toggleRecording(true);
        }
      }),
      events.shortcutRewrite.listen(() => {
        if (app.recordingMode === "meeting") return;
        if (!isStartingRecording && !isStopping) {
          if (!isDictating) sessionMode = "rewrite";
          void toggleRecording(true);
        }
      }),
      events.shortcutPttStart.listen(() => {
        // Push-to-talk only starts from a fully idle machine: a meeting (or
        // an already-running dictation) must not be interrupted.
        if (app.recordingMode === "idle" && !isStartingRecording && !isStopping) {
          sessionMode = "insert";
          void toggleRecording(true);
        }
      }),
      events.shortcutPttStop.listen(() => {
        if (isDictating && !isStopping) void toggleRecording(true);
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

  /**
   * Toggles the dictation recording session on or off.
   * Prevents starting if another session (e.g. meeting) is currently active.
   * When stopping, waits for the model to finish draining and optionally
   * runs Polish on the assembled text before pasting or storing.
   *
   * @param {boolean} fromShortcut - Whether the toggle was triggered via a global keyboard shortcut.
   */
  async function toggleRecording(fromShortcut = false) {
    if (isStartingRecording || isStopping) return;

    if (!isDictating && app.recordingMode !== "idle") {
      // Cannot start dictation while a meeting (or anything else) is recording.
      return;
    }

    if (isDictating) {
      isStopping = true;

      // Polish keeps running after the recording state ends (it's an LLM
      // call over the finalized text); hold the pill open now, before the
      // state machine leaves the recording state, so it doesn't flash shut
      // and reopen. Always released below, even on error, so a failed
      // polish or paste never leaves a zombie pill.
      const holdForPolish = app.settings.dictation_polish_enabled;
      const sessionFocusedApp = focusedApp;
      const sessionRewriteOf = rewriteOf;
      if (holdForPolish) {
        try {
          await pillHold("polishing");
        } catch (e) {
          console.warn("Pill hold failed:", e);
        }
      }

      try {
        await stopStreamingTranscription();

        const finalized = await finalizeDictationText(
          transcript,
          sessionFocusedApp,
          sessionRewriteOf,
        );
        if (finalized.warning) {
          statusMessage = finalized.warning;
        }

        const saved = await saveToHistory(finalized.text);

        if (finalized.text) {
          if (fromShortcut && app.settings.auto_paste) {
            try {
              await pasteText(
                finalized.text,
                app.settings.paste_delay_ms,
                app.settings.paste_method,
              );
              scheduleLearnFromEdit(finalized.text);
            } catch (e) {
              const message = errorMessage(e);
              statusMessage = accessibilityPasteFailureMessage(message);
              // A shortcut dictation runs from another app, so the status
              // banner above is likely not on screen: also notify outside
              // the window (SOU-053). Best-effort: a notification failure
              // must not mask the paste failure itself.
              try {
                await notifyPasteFailed(message, saved);
              } catch (notifyError) {
                console.warn("Paste failure notification failed:", notifyError);
              }
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
        clearSessionContext();
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

    cancelLearnFromEditPoll();
    if (!fromShortcut) sessionMode = "insert";
    transcript = "";
    tentative = "";
    statusMessage = "";
    isStartingRecording = true;
    sessionGeneration += 1;
    const generation = sessionGeneration;

    try {
      await captureStartContext();
      await startStreamingTranscription((segment: TranscriptionSegment) => {
        if (generation !== sessionGeneration) return; // stale session
        if (!segment.is_final) {
          tentative = segment.text;
          return;
        }
        tentative = "";
        transcript += segmentGap(transcript, segment.text) + segment.text;
      });
    } catch (e) {
      statusMessage = errorMessage(e);
      clearSessionContext();
    } finally {
      isStartingRecording = false;
    }
  }

  /** The backend aborted the recording session (machine went to Error). */
  function handleRecordingAborted() {
    const sessionFocusedApp = focusedApp;
    const sessionRewriteOf = rewriteOf;
    sessionGeneration += 1; // cut off in-flight segments from the dead session
    isStartingRecording = false;
    isStopping = false;
    tentative = "";
    cancelLearnFromEditPoll();
    if (transcript.trim()) {
      void finalizeDictationText(transcript, sessionFocusedApp, sessionRewriteOf).then(({ text, warning }) => {
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
    clearSessionContext();
  }

  return {
    get app() { return app; },
    get isStartingRecording() { return isStartingRecording; },
    get isStopping() { return isStopping; },
    get transcript() { return transcript; },
    get tentative() { return tentative; },
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

/** The native HUD asked to stop the active dictation; run the full stop
 * pipeline (polish + paste) so HUD stop matches the shortcut (SOU-046).
 * Stop-only: a no-op when not dictating, so this cannot start a session
 * or take down a meeting (SOU-044). */
export function notifyDictationStopRequested() {
  if (instance && instance.app.recordingMode === "dictation" && !instance.isStopping) {
    void instance.toggleRecording(true);
  }
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
