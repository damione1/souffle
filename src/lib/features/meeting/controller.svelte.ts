import {
  deleteMeeting as removeMeeting,
  getMeeting,
  resumeMeetingRecording,
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

  let isRecordingMeeting = $state(false);
  let meetingTitle = $state("");
  let liveMeetingSegments = $state<TranscriptionSegment[]>([]);

  let meeting = $state<MeetingTranscript | null>(null);
  let isLoadingMeeting = $state(false);
  let canResumeRecording = $derived(
    Boolean(meeting?.id) && !isRecordingMeeting && !isLoadingMeeting && !isSummarizing,
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
      const title = meetingTitle.trim() || defaultMeetingTitle();
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

      isRecordingMeeting = true;
      app.isRecording = true;
      app.recordingMode = "meeting";
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
      };
      meetingTitle = "";
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

      isRecordingMeeting = true;
      app.isRecording = true;
      app.recordingMode = "meeting";
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

      isRecordingMeeting = false;
      app.isRecording = false;
      app.recordingMode = "idle";
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  /** Clear controller state for starting a fresh meeting. */
  function startNew() {
    meeting = null;
    liveMeetingSegments = [];
    meetingTitle = "";
    statusMessage = "";
    summaryStream = "";
    app.currentMeetingId = null;
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

  async function deleteMeeting() {
    if (!meeting || !meeting.id) return;
    try {
      await removeMeeting(meeting.id);
      meeting = null;
      app.currentMeetingId = null;
      app.currentView = "meeting-history";
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
    get meetingTitle() { return meetingTitle; },
    set meetingTitle(value: string) { meetingTitle = value; },
    get liveMeetingSegments() { return liveMeetingSegments; },
    get meeting() { return meeting; },
    get isLoadingMeeting() { return isLoadingMeeting; },
    get canResumeRecording() { return canResumeRecording; },
    mount,
    onMeetingSelectionChange,
    checkOllama,
    startRecording,
    resumeRecording,
    stopRecording,
    startNew,
    summarizeMeeting,
    deleteMeeting,
  };
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
