import {
  deleteMeeting as removeMeeting,
  getMeeting,
  renameMeeting as applyMeetingRename,
  resumeMeetingRecording,
  saveEditedTranscript,
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

  let isRecordingMeeting = $derived(
    app.machineState.state === "recording_meeting"
    || (app.machineState.state === "stopping"
        && typeof app.machineState.data?.was_recording === "object"),
  );
  let liveMeetingSegments = $state<TranscriptionSegment[]>([]);

  let meeting = $state<MeetingTranscript | null>(null);
  let isLoadingMeeting = $state(false);
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
    } catch (e) {
      statusMessage = errorMessage(e);
    } finally {
      isLoadingMeeting = false;
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
    try {
      const id = await stopMeetingRecording();

      // Load the completed meeting BEFORE clearing recording flags.
      // This prevents the view from flashing to "new meeting" mode
      // during the transition (meeting stays non-null the whole time).
      app.currentMeetingId = id;
      meeting = await getMeeting(id);
      syncSelectedModel(meeting.summary_model);
      liveMeetingSegments = [];
    } catch (e) {
      statusMessage = errorMessage(e);
    }
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
    isSummarizing = true;
    summaryStream = "";
    statusMessage = "";

    try {
      await runMeetingSummary(meeting.id, selectedModel, (progress: SummarizeProgress) => {
        summaryStream += progress.text;
        if (progress.done) {
          isSummarizing = false;
          void getMeeting(meeting!.id).then((loadedMeeting) => {
            meeting = loadedMeeting;
            syncSelectedModel(meeting.summary_model);
          });
        }
      });
    } catch (e) {
      statusMessage = errorMessage(e);
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
    get canResumeRecording() { return canResumeRecording; },
    get isEditingTranscript() { return isEditingTranscript; },
    get editedTranscriptDraft() { return editedTranscriptDraft; },
    set editedTranscriptDraft(value: string) { editedTranscriptDraft = value; },
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
  };
}

/** Called from the global StateChanged listener when a meeting session
 * is aborted by the backend. No-op if the controller was never created. */
export function notifyMeetingAborted() {
  instance?.handleRecordingAborted();
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
