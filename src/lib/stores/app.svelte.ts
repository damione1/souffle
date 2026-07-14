import type {
  AppSettings,
  AppStateMachine,
  PipelineError,
  SystemAudioStatus,
  TranscriptionHealth,
  TranscriptionRuntimePhase,
  MeetingStartNudge,
  UpcomingMeeting,
} from "../types";
import type { TranscriptionModelOperationState } from "../features/transcription/state";

// Settings sheet visibility (the app is otherwise a single home surface)
let settingsOpen = $state(false);

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

// Latest pipeline health snapshot while recording (cleared when recording ends)
let transcriptionHealth = $state<TranscriptionHealth | null>(null);

// System-audio capture status for the current meeting session
let systemAudioStatus = $state<SystemAudioStatus | null>(null);

// Calendar reminder awaiting the user's decision (drives the home banner)
let upcomingMeeting = $state<UpcomingMeeting | null>(null);

// Coalesced smart-start nudge (process / audio / calendar)
let meetingStartNudge = $state<MeetingStartNudge | null>(null);

// First-run onboarding (model not downloaded yet) — derived at bootstrap
let showOnboarding = $state(false);

// Last pipeline error surfaced by the backend (dismissable)
let pipelineError = $state<PipelineError | null>(null);

// Download progress — pure UI state, not derivable from machine
let downloadFile = $state("");
let downloadCompletedFiles = $state(0);
let downloadTotalFiles = $state(0);
let downloadedBytes = $state(0);
let downloadTotalBytes = $state<number | null>(null);

// Settings with defaults
let settings = $state<AppSettings>({
  theme: "dark",
  locale: "",
  auto_paste: false,
  paste_delay_ms: 100,
  paste_method: "clipboard",
  ollama_url: "http://localhost:11434",
  ollama_model: "",
  debug_transcription: false,
  log_level: "info",
  audio_device: null,
  clamshell_audio_device: null,
  transcription_engine_id: "",
  transcription_model_id: "",
  transcription_backend_id: "",
  vad_enabled: true,
  filler_removal: true,
  stutter_collapse: false,
  dictionary_correction: true,
  capture_system_audio: true,
  calendar_integration_enabled: false,
  calendar_selected_ids: [],
  calendar_reminder_minutes: 2,
  calendar_autostart_enabled: true,
  feedback_sounds_enabled: true,
  feedback_sounds_volume: 70,
  model_unload_timeout_minutes: 0,
  meeting_autostop_enabled: true,
  meeting_autostop_minutes: 10,
  meeting_max_duration_minutes: 240,
  meeting_smart_start_enabled: true,
  meeting_smart_stop_enabled: true,
  meeting_audio_retention: "off",
  meeting_transcription_language: "auto",
  dictation_polish_enabled: false,
  dictation_polish_template_id: "email",
  dictation_polish_templates: [
    { id: "email", label: "Professional email", prompt: "Rewrite as email." },
    { id: "bullets", label: "Bullet points", prompt: "Use bullets." },
    { id: "no_fillers", label: "Remove fillers", prompt: "Remove fillers." },
  ],
  default_summary_template_id: "default",
  summary_templates: [
    { id: "default", name: "Default", prompt: "" },
    { id: "detailed_minutes", name: "Detailed minutes", prompt: "" },
    { id: "brief_overview", name: "Brief overview", prompt: "" },
  ],
  last_seen_version: "",
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
    get settingsOpen() { return settingsOpen; },
    set settingsOpen(v: boolean) { settingsOpen = v; },

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
      // Health snapshots only make sense while recording
      if (deriveRecordingMode(s) === "idle") {
        transcriptionHealth = null;
        systemAudioStatus = null;
      }
    },

    get transcriptionHealth() { return transcriptionHealth; },
    set transcriptionHealth(h: TranscriptionHealth | null) { transcriptionHealth = h; },

    get systemAudioStatus() { return systemAudioStatus; },
    set systemAudioStatus(s: SystemAudioStatus | null) { systemAudioStatus = s; },
    get upcomingMeeting() { return upcomingMeeting; },
    set upcomingMeeting(u: UpcomingMeeting | null) { upcomingMeeting = u; },
    get meetingStartNudge() { return meetingStartNudge; },
    set meetingStartNudge(n: MeetingStartNudge | null) { meetingStartNudge = n; },

    get showOnboarding() { return showOnboarding; },
    set showOnboarding(v: boolean) { showOnboarding = v; },

    get pipelineError() { return pipelineError; },
    set pipelineError(e: PipelineError | null) { pipelineError = e; },

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

    get downloadedBytes() { return downloadedBytes; },
    set downloadedBytes(v: number) { downloadedBytes = v; },

    get downloadTotalBytes() { return downloadTotalBytes; },
    set downloadTotalBytes(v: number | null) { downloadTotalBytes = v; },

    /** Open a meeting's detail view */
    openMeeting(id: string) {
      currentMeetingId = id;
    },
  };
}
