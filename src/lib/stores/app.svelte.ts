import type { AppSettings, AppStateMachine, AppView, TranscriptionRuntimePhase } from "../types";
import type { TranscriptionModelOperationState } from "../features/transcription/state";

// Current view
let currentView = $state<AppView>("transcription");

// Current meeting ID (when viewing a specific meeting)
let currentMeetingId = $state<string | null>(null);

// Selected audio device (persisted across tab switches)
let selectedDevice = $state("");

// Unified state machine from backend — single source of truth
let machineState = $state<AppStateMachine>({ state: "idle" });

// Transcription runtime phase for the *selected* profile.
// This can differ from machineState when the user selects a different
// model in settings (machine stays Ready with old profile while the UI
// shows "download required" for the new one).
let transcriptionRuntimePhase = $state<TranscriptionRuntimePhase>("download_required");

// Download progress — pure UI state, not derivable from machine
let downloadFile = $state("");
let downloadCompletedFiles = $state(0);
let downloadTotalFiles = $state(0);

// Settings with defaults
let settings = $state<AppSettings>({
  theme: "dark",
  locale: "",
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

function deriveRecordingMode(state: AppStateMachine): "idle" | "dictation" | "meeting" {
  switch (state.state) {
    case "recording_dictation":
      return "dictation";
    case "recording_meeting":
      return "meeting";
    case "stopping":
      return typeof state.data.was_recording === "object" ? "meeting" : "dictation";
    default:
      return "idle";
  }
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

function deriveModelOperationState(state: AppStateMachine): TranscriptionModelOperationState {
  switch (state.state) {
    case "downloading": return "downloading";
    case "loading": return "loading";
    default: return "idle";
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
      // Sync runtime phase from machine when no model operation is in progress.
      // During download/load the phase is managed by runtime.ts callbacks.
      if (deriveModelOperationState(s) === "idle") {
        transcriptionRuntimePhase = deriveRuntimePhase(s);
      }
    },

    // Derived from machineState — no separate $state
    get isRecording() {
      const s = machineState.state;
      return s === "recording_dictation" || s === "recording_meeting" || s === "stopping";
    },
    get recordingMode() { return deriveRecordingMode(machineState); },
    get transcriptionModelOperationState(): TranscriptionModelOperationState {
      return deriveModelOperationState(machineState);
    },

    get transcriptionRuntimePhase() { return transcriptionRuntimePhase; },
    set transcriptionRuntimePhase(v: TranscriptionRuntimePhase) { transcriptionRuntimePhase = v; },

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
