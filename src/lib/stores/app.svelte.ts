import type { AppSettings, AppStateMachine, AppView, TranscriptionRuntimePhase } from "../types";

export type RecordingMode = "idle" | "dictation" | "meeting";
export type TranscriptionModelOperationState = "idle" | "downloading" | "loading";

// Current view
let currentView = $state<AppView>("transcription");

// Current meeting ID (when viewing a specific meeting)
let currentMeetingId = $state<string | null>(null);

// Selected audio device (persisted across tab switches)
let selectedDevice = $state("");

// Unified state machine from backend
let machineState = $state<AppStateMachine>({ state: "idle" });

// Recording state — shared across views (kept for backward compat, now derived from machineState)
let isRecording = $state(false);
let recordingMode = $state<RecordingMode>("idle");

// Transcription runtime state — shared across views
let transcriptionRuntimePhase = $state<TranscriptionRuntimePhase>("download_required");
let transcriptionModelOperationState = $state<TranscriptionModelOperationState>("idle");
let downloadFile = $state("");
let downloadCompletedFiles = $state(0);
let downloadTotalFiles = $state(0);

// Settings with defaults
let settings = $state<AppSettings>({
  theme: "dark",
  auto_paste: false,
  paste_delay_ms: 100,
  ollama_url: "http://localhost:11434",
  ollama_model: "",
  debug_transcription: false,
  audio_device: null,
  transcription_engine_id: "",
  transcription_model_id: "",
  transcription_backend_id: "",
});

function deriveRecordingMode(state: AppStateMachine): RecordingMode {
  switch (state.state) {
    case "recording_dictation":
      return "dictation";
    case "recording_meeting":
    case "stopping":
      return state.state === "stopping"
        ? (typeof state.data.was_recording === "object" ? "meeting" : "dictation")
        : "meeting";
    default:
      return "idle";
  }
}

function deriveIsRecording(state: AppStateMachine): boolean {
  return state.state === "recording_dictation"
    || state.state === "recording_meeting"
    || state.state === "stopping";
}

function deriveRuntimePhase(state: AppStateMachine): TranscriptionRuntimePhase {
  switch (state.state) {
    case "idle":
    case "downloading":
      return "download_required";
    case "downloaded":
    case "loading":
      return "load_required";
    case "ready":
    case "recording_dictation":
    case "recording_meeting":
    case "stopping":
    case "unloading":
      return "ready";
    case "error":
      if (state.data.recovery === "retry_from_idle") return "download_required";
      if (typeof state.data.recovery === "object") {
        if ("retry_from_downloaded" in state.data.recovery) return "load_required";
        if ("retry_from_ready" in state.data.recovery) return "ready";
      }
      return "download_required";
  }
}

export function getAppState() {
  return {
    get currentView() { return currentView; },
    set currentView(v: AppView) { currentView = v; },

    get currentMeetingId() { return currentMeetingId; },
    set currentMeetingId(id: string | null) { currentMeetingId = id; },

    get settings() { return settings; },
    set settings(s: AppSettings) { settings = s; },

    get selectedDevice() { return selectedDevice; },
    set selectedDevice(d: string) { selectedDevice = d; },

    get machineState() { return machineState; },
    set machineState(s: AppStateMachine) {
      machineState = s;
      // Sync derived fields from machine state
      isRecording = deriveIsRecording(s);
      recordingMode = deriveRecordingMode(s);
      // Only update runtime phase from machine if not in a transient operation state
      if (transcriptionModelOperationState === "idle") {
        transcriptionRuntimePhase = deriveRuntimePhase(s);
      }
    },

    get isRecording() { return isRecording; },
    set isRecording(v: boolean) { isRecording = v; },

    get recordingMode() { return recordingMode; },
    set recordingMode(m: RecordingMode) { recordingMode = m; },

    get transcriptionRuntimePhase() { return transcriptionRuntimePhase; },
    set transcriptionRuntimePhase(v: TranscriptionRuntimePhase) { transcriptionRuntimePhase = v; },

    get transcriptionModelOperationState() { return transcriptionModelOperationState; },
    set transcriptionModelOperationState(v: TranscriptionModelOperationState) { transcriptionModelOperationState = v; },

    get downloadFile() { return downloadFile; },
    set downloadFile(v: string) { downloadFile = v; },

    get downloadCompletedFiles() { return downloadCompletedFiles; },
    set downloadCompletedFiles(v: number) { downloadCompletedFiles = v; },

    get downloadTotalFiles() { return downloadTotalFiles; },
    set downloadTotalFiles(v: number) { downloadTotalFiles = v; },

    /** Navigate to a specific meeting's detail page */
    openMeeting(id: string) {
      currentMeetingId = id;
      currentView = "meeting";
    },

    /** Navigate to meeting tab in "new meeting" mode */
    newMeeting() {
      currentMeetingId = null;
      currentView = "meeting";
    },
  };
}
