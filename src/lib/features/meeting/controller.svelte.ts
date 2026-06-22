import {
  deleteMeeting as removeMeeting,
  getMeeting,
  renameMeeting as applyMeetingRename,
  resumeMeetingRecording,
  saveEditedTranscript,
  saveMeetingNotes,
  startMeetingRecording,
  stopMeetingRecording,
  summarizeMeeting as runMeetingSummary,
} from "../../api/meetings";
import { getOllamaStatus } from "../../api/ollama";
import { getTranscriptionCatalog } from "../../api/transcription";
import { getAppState } from "../../stores/app.svelte";
import type { MeetingTranscript, OllamaModelDescriptor, SummarizeProgress, TranscriptionCatalog, TranscriptionSegment } from "../../types";
import { errorMessage } from "../../utils";
import { toSelectedTranscriptionProfile } from "../transcription/catalog";

function defaultMeetingTitle(): string {
  return `Meeting ${new Date().toLocaleDateString()}`;
}

function createMeetingControllerInstance() {
  const app = getAppState();

  let statusMessage = $state("");
  let ollamaAvailable = $state(false);
  let summaryModels = $state<OllamaModelDescriptor[]>([]);
  let selectedModel = $state("");
  let isSummarizing = $state(false);
  let summaryStream = $state("");
  let transcriptionCatalog = $state<TranscriptionCatalog | null>(null);

  let isEditingTranscript = $state(false);
  let editedTranscriptDraft = $state("");

  const NOTES_DEBOUNCE_MS = 800;
  let notesDraft = $state("");
  let notesSaveState = $state<"idle" | "pending" | "saved">("idle");
  let notesTimer: ReturnType<typeof setTimeout> | null = null;

  let isRecordingMeeting = $derived(
    app.machineState.state === "recording_meeting"
    || (app.machineState.state === "stopping"
        && typeof app.machineState.data?.was_recording === "object"),
  );
  let liveMeetingSegments = $state<TranscriptionSegment[]>([]);

  let meeting = $state<MeetingTranscript | null>(null);
  let isLoadingMeeting = $state(false);
  // True from the moment Stop is clicked until the backend finishes draining +
  // saving (machine leaves "stopping"). Drives the Stop button's spinner and
  // guards against double-stop. `stopRequested` covers the brief gap before the
  // machine reports "stopping".
  let stopRequested = $state(false);
  let isStopping = $derived(stopRequested || app.machineState.state === "stopping");
  let canResumeRecording = $derived(
    Boolean(meeting?.id)
    && !isRecordingMeeting
    && !isLoadingMeeting
    && !isSummarizing
    && app.transcriptionRuntimePhase === "ready",
  );

  async function mount() {
    await Promise.all([checkOllama(), loadTranscriptionCatalog()]);
  }

  async function loadTranscriptionCatalog() {
    try {
      transcriptionCatalog = await getTranscriptionCatalog();
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  async function onMeetingSelectionChange(id: string | null) {
    if (id && (!meeting || meeting.id !== id) && !isRecordingMeeting) {
      await loadMeeting(id);
    }
  }

  function syncSelectedModel(preferredModel?: string | null) {
    if (summaryModels.length === 0) {
      selectedModel = "";
      return;
    }
    if (preferredModel && summaryModels.some((model) => model.id === preferredModel)) {
      selectedModel = preferredModel;
      return;
    }
    if (selectedModel && summaryModels.some((model) => model.id === selectedModel)) return;
    if (app.settings.ollama_model && summaryModels.some((model) => model.id === app.settings.ollama_model)) {
      selectedModel = app.settings.ollama_model;
      return;
    }
    selectedModel = summaryModels[0].id;
  }

  async function loadMeeting(id: string) {
    isLoadingMeeting = true;
    try {
      meeting = await getMeeting(id);
      syncSelectedModel(meeting.summary_model);
      notesDraft = meeting.notes ?? "";
      notesSaveState = "idle";
    } catch (e) {
      statusMessage = errorMessage(e);
    } finally {
      isLoadingMeeting = false;
    }
  }

  /** The meeting id to write notes against: the accumulator id while
   * recording (the row doesn't exist in the DB yet), the row id after. */
  function notesTargetId(): string {
    const machineState = app.machineState;
    if (machineState.state === "recording_meeting") return machineState.data.meeting_id;
    return meeting?.id ?? "";
  }

  function onNotesChange(value: string) {
    notesDraft = value;
    notesSaveState = "pending";
    if (notesTimer) clearTimeout(notesTimer);
    notesTimer = setTimeout(() => void flushNotes(), NOTES_DEBOUNCE_MS);
  }

  async function flushNotes() {
    if (notesTimer) {
      clearTimeout(notesTimer);
      notesTimer = null;
    }
    if (notesSaveState !== "pending") return;
    const id = notesTargetId();
    if (!id) return;
    try {
      await saveMeetingNotes(id, notesDraft.trim() || null);
      notesSaveState = "saved";
    } catch (e) {
      statusMessage = errorMessage(e);
      notesSaveState = "idle";
    }
  }

  async function checkOllama() {
    try {
      const status = await getOllamaStatus();
      ollamaAvailable = status.available;
      summaryModels = status.models.filter((model) => model.can_summarize);
      syncSelectedModel(meeting?.summary_model);
    } catch {
      ollamaAvailable = false;
      summaryModels = [];
    }
  }

  async function startRecording() {
    try {
      const title = defaultMeetingTitle();
      liveMeetingSegments = [];
      statusMessage = "";
      summaryStream = "";
      meeting = null;
      notesDraft = "";
      notesSaveState = "idle";
      const transcriptionProfile = toSelectedTranscriptionProfile(
        transcriptionCatalog,
        app.settings.transcription_engine_id,
        app.settings.transcription_model_id,
        app.settings.transcription_backend_id,
      );

      await startMeetingRecording(title, (segment) => {
        if (!segment.is_final || !segment.text) return;
        liveMeetingSegments = [...liveMeetingSegments, segment];
      });

      meeting = {
        id: "",
        title,
        started_at: new Date().toISOString(),
        ended_at: null,
        duration_seconds: 0,
        transcription_profile: transcriptionProfile,
        recording_sessions: [],
        segments: [],
        summary: null,
        summary_is_stale: false,
        summary_model: null,
        summary_generated_at: null,
        edited_transcript: null,
      };
    } catch (e) {
      statusMessage = errorMessage(e);
      liveMeetingSegments = [];
    }
  }

  async function resumeRecording() {
    if (!meeting || !meeting.id) return;

    try {
      liveMeetingSegments = [];
      statusMessage = "";
      summaryStream = "";

      await resumeMeetingRecording(meeting.id, (segment) => {
        if (!segment.is_final || !segment.text) return;
        liveMeetingSegments = [...liveMeetingSegments, segment];
      });
    } catch (e) {
      statusMessage = errorMessage(e);
      liveMeetingSegments = [];
    }
  }

  async function stopRecording() {
    if (isStopping) return; // guard against double-stop
    stopRequested = true;
    try {
      // Unsaved notes must reach the accumulator before it is persisted.
      await flushNotes();
      // Decoupled stop: returns the id immediately; the backend drains and
      // saves in the background and emits `meetingFinalized` when done.
      const id = await stopMeetingRecording();
      app.currentMeetingId = id;

      // Optimistically load the partially-persisted meeting so the detail view
      // has data the instant the machine flips to idle. The header + most
      // segments are already on disk from incremental persistence; the
      // `meetingFinalized` event reloads the authoritative version.
      try {
        meeting = await getMeeting(id);
        syncSelectedModel(meeting.summary_model);
        notesDraft = meeting.notes ?? "";
      } catch {
        // Header may not be queryable yet in a rare race; finalize reloads it.
      }
    } catch (e) {
      statusMessage = errorMessage(e);
    } finally {
      stopRequested = false;
    }
  }

  /** The backend finished draining + saving a stopped meeting. Reload the
   * now-complete record (ended_at, duration, all segments) and drop the live
   * buffer. No-op if the user already navigated elsewhere. */
  function handleMeetingFinalized(id: string) {
    if (app.currentMeetingId !== id && meeting?.id !== id) return;
    liveMeetingSegments = [];
    void loadMeeting(id);
  }

  /** The backend aborted the recording session (machine went to Error).
   * The backend salvages the accumulated meeting to history before failing. */
  function handleRecordingAborted() {
    liveMeetingSegments = [];
    meeting = null;
    app.currentMeetingId = null;
    statusMessage =
      "Recording was interrupted — the meeting recorded so far was saved to history.";
  }

  /** Leave the detail view: clear the open meeting and return to the list. */
  function closeMeeting() {
    void flushNotes();
    meeting = null;
    liveMeetingSegments = [];
    statusMessage = "";
    summaryStream = "";
    app.currentMeetingId = null;
  }

  /** Rename the open meeting (works while recording too). */
  async function renameMeeting(title: string) {
    if (!meeting) return;
    const trimmed = title.trim();
    if (!trimmed || trimmed === meeting.title) return;
    try {
      // A live meeting that hasn't been stopped yet has the accumulator id
      // in the machine state, not in the (placeholder) meeting object.
      const machineState = app.machineState;
      const id = meeting.id
        || (machineState.state === "recording_meeting" ? machineState.data.meeting_id : "");
      if (!id) return;
      await applyMeetingRename(id, trimmed);
      meeting = { ...meeting, title: trimmed };
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  async function summarizeMeeting() {
    if (!selectedModel || !meeting || !meeting.id) return;
    const meetingId = meeting.id;
    isSummarizing = true;
    summaryStream = "";
    statusMessage = "";

    try {
      // Stream tokens for live preview only. Completion is driven by the
      // command resolving (it returns after the summary is saved), NOT by the
      // streaming `done` flag — a dropped final chunk must not leave the UI
      // stuck "Generating…".
      await runMeetingSummary(meetingId, selectedModel, (progress: SummarizeProgress) => {
        summaryStream += progress.text;
      });
      const loadedMeeting = await getMeeting(meetingId);
      meeting = loadedMeeting;
      syncSelectedModel(meeting.summary_model);
    } catch (e) {
      statusMessage = errorMessage(e);
    } finally {
      isSummarizing = false;
    }
  }

  function startEditingTranscript() {
    if (!meeting) return;
    // Use edited_transcript if available, otherwise join segments
    editedTranscriptDraft = meeting.edited_transcript
      ?? meeting.segments.map((s) => s.text).join(" ");
    isEditingTranscript = true;
  }

  function cancelEditingTranscript() {
    isEditingTranscript = false;
    editedTranscriptDraft = "";
  }

  async function saveTranscriptEdit() {
    if (!meeting || !meeting.id) return;
    try {
      const textToSave = editedTranscriptDraft.trim() || null;
      await saveEditedTranscript(meeting.id, textToSave);
      meeting = await getMeeting(meeting.id);
      isEditingTranscript = false;
      editedTranscriptDraft = "";
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  async function saveTranscriptAndSummarize() {
    await saveTranscriptEdit();
    if (!isEditingTranscript) {
      await summarizeMeeting();
    }
  }

  async function resetEditedTranscript() {
    if (!meeting || !meeting.id) return;
    try {
      await saveEditedTranscript(meeting.id, null);
      meeting = await getMeeting(meeting.id);
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  async function deleteMeeting() {
    if (!meeting || !meeting.id) return;
    try {
      await removeMeeting(meeting.id);
      closeMeeting();
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  return {
    get app() { return app; },
    get statusMessage() { return statusMessage; },
    get ollamaAvailable() { return ollamaAvailable; },
    get summaryModels() { return summaryModels; },
    get selectedModel() { return selectedModel; },
    set selectedModel(modelId: string) { selectedModel = modelId; },
    get isSummarizing() { return isSummarizing; },
    get summaryStream() { return summaryStream; },
    get isRecordingMeeting() { return isRecordingMeeting; },
    get liveMeetingSegments() { return liveMeetingSegments; },
    get meeting() { return meeting; },
    get isLoadingMeeting() { return isLoadingMeeting; },
    get isStopping() { return isStopping; },
    get canResumeRecording() { return canResumeRecording; },
    get isEditingTranscript() { return isEditingTranscript; },
    get editedTranscriptDraft() { return editedTranscriptDraft; },
    set editedTranscriptDraft(value: string) { editedTranscriptDraft = value; },
    get notesDraft() { return notesDraft; },
    get notesSaveState() { return notesSaveState; },
    onNotesChange,
    flushNotes,
    mount,
    onMeetingSelectionChange,
    checkOllama,
    startRecording,
    resumeRecording,
    stopRecording,
    closeMeeting,
    renameMeeting,
    summarizeMeeting,
    deleteMeeting,
    startEditingTranscript,
    cancelEditingTranscript,
    saveTranscriptEdit,
    saveTranscriptAndSummarize,
    resetEditedTranscript,
    handleRecordingAborted,
    handleMeetingFinalized,
  };
}

/** Called from the global StateChanged listener when a meeting session
 * is aborted by the backend. No-op if the controller was never created. */
export function notifyMeetingAborted() {
  instance?.handleRecordingAborted();
}

/** Called from the global MeetingFinalized listener when a stopped meeting has
 * been fully drained and saved in the background. */
export function notifyMeetingFinalized(id: string) {
  instance?.handleMeetingFinalized(id);
}

/** The floating pill (or tray) asked to stop the active meeting; run the
 * full stop pipeline so the meeting is saved normally. */
export function notifyMeetingStopRequested() {
  if (instance?.isRecordingMeeting) {
    void instance.stopRecording();
  }
}

// Singleton: survives view mount/unmount cycles so liveMeetingSegments
// and recording state are never lost when the user switches tabs.
let instance: ReturnType<typeof createMeetingControllerInstance> | null = null;

export function createMeetingController() {
  if (!instance) {
    instance = createMeetingControllerInstance();
  }
  return instance;
}

/** Reset the singleton for testing. */
export function resetMeetingControllerForTest() {
  instance = null;
}
