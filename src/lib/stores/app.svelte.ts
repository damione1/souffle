import type { SettingsAnchor } from "../features/settings/anchors";
import type {
  PermissionStatus,
  AppSettings,
  AppStateMachine,
  ModifierTapStatus,
  PipelineError,
  SnippetEntry,
  SystemAudioStatus,
  TranscriptionHealth,
  TranscriptionProfile,
  TranscriptionRuntimePhase,
  UpcomingMeeting,
} from "../types";
import type { TranscriptionModelOperationState } from "../features/transcription/state";

// Settings sheet visibility (the app is otherwise a single home surface)
let settingsOpen = $state(false);
let appPermissions = $state<PermissionStatus | null>(null);

// Tab to land on the next time settings opens (e.g. a "set up calendar" CTA
// deep-linking into the meetings tab). Consumed once by SettingsView.
let settingsInitialAnchor = $state<SettingsAnchor | null>(null);

// Permissions repair panel, mounted once in App so banners can open it
// without going through Settings → System → Review (SOU-089).
let permissionsPanelOpen = $state(false);

// Voice snippets (SOU-035). Loaded once at startup and written through by
// the settings sheet, so finalizing a dictation matches against this list
// without an IPC round-trip.
let snippets = $state<SnippetEntry[]>([]);

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

// Wall-clock start of the current recording session (dictation or meeting),
// captured when the backend machine enters a recording state. The live
// elapsed counter derives from this instead of counting timer ticks, which
// WebKit drops while the window sits in the background during a meeting.
// Module-level, so it also survives a remount of the live session card.
let recordingStartedAtMs = $state<number | null>(null);

// System-audio capture status for the current meeting session
let systemAudioStatus = $state<SystemAudioStatus | null>(null);

// Native single-key PTT CGEventTap install status (SOU-116)
let modifierTapStatus = $state<ModifierTapStatus | null>(null);

// Accelerators the CGEventTap handles, read once from the backend so the
// settings UI does not keep its own copy of the list (SOU-139).
let nativeShortcuts = $state<string[]>([]);

// Calendar reminder awaiting the user's decision (drives the home banner)
let upcomingMeeting = $state<UpcomingMeeting | null>(null);

// First-run setup wizard (permissions, mic, model, shortcut)
let showOnboarding = $state(false);

// Last pipeline error surfaced by the backend (dismissable)
let pipelineError = $state<PipelineError | null>(null);

// Download progress — pure UI state, not derivable from machine
let downloadFile = $state("");
let downloadCompletedFiles = $state(0);
let downloadTotalFiles = $state(0);
let downloadedBytes = $state(0);
let downloadTotalBytes = $state<number | null>(null);

// Settings with defaults matching AppSettings::default() in src-tauri/src/settings.rs.
// getSettings() overwrites these on every successful bootstrap; these are only
// active if the backend is unreachable on first launch (onboarding flow).
let settings = $state<AppSettings>({
  theme: "light",
  locale: "",
  auto_paste: false,
  paste_delay_ms: 100,
  paste_method: "clipboard",
  ollama_url: "http://localhost:11434",
  summary_provider: "auto",
  ollama_model: "",
  debug_transcription: false,
  log_level: "info",
  audio_device: null,
  clamshell_audio_device: null,
  input_priority: { priorities: [], hidden: [], known: [] },
  allow_bluetooth_mic: false,
  // Matches KYUTAI_ENGINE_ID / KYUTAI_MODEL_ID / CANDLE_BACKEND_ID in engine/mod.rs
  transcription_engine_id: "kyutai",
  transcription_model_id: "stt-1b-en_fr",
  transcription_backend_id: "candle",
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
  pill_hidden: false,
  feedback_sounds_volume: 70,
  model_unload_timeout_minutes: 60,
  meeting_autostop_enabled: true,
  meeting_autostop_minutes: 10,
  meeting_max_duration_minutes: 240,
  autostart_enabled: false,
  meeting_audio_retention: "off",
  meeting_transcription_language: "auto",
  dictation_polish_enabled: true,
  dictation_polish_template_id: "clean",
  // Fallback only, used when getSettings() has not yet succeeded. Prompts
  // are empty on purpose: merge_polish_templates keeps a stored empty prompt
  // (it is not in the superseded-builtin list), but effective_template_prompt
  // falls back to the shipped defaults at polish time, so a bootstrap-failure
  // path cannot persist the old stub one-liners as the live polish text.
  dictation_polish_templates: [
    { id: "clean", label: "Clean up", prompt: "" },
    { id: "email", label: "Professional email", prompt: "" },
    { id: "bullets", label: "Bullet points", prompt: "" },
    { id: "no_fillers", label: "Remove fillers", prompt: "" },
  ],
  auto_update_check_enabled: true,
  dictation_learn_from_edit: true,
  default_summary_template_id: "default",
  // Stub prompts: same rationale as dictation_polish_templates above.
  summary_templates: [
    { id: "default", name: "Default", prompt: "" },
    { id: "detailed_minutes", name: "Detailed minutes", prompt: "" },
    { id: "brief_overview", name: "Brief overview", prompt: "" },
  ],
  last_seen_version: "",
  dictation_ceiling_seconds: 300,
});

export function deriveRecordingMode(state: AppStateMachine): "idle" | "dictation" | "meeting" {
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

/** The profile the backend is currently busy with, when it tracks one. */
export function machineProfile(state: AppStateMachine): TranscriptionProfile | null {
  switch (state.state) {
    case "idle":
      return null;
    case "error": {
      const recovery = state.data.recovery;
      if (typeof recovery === "object") {
        if ("retry_from_downloaded" in recovery) return recovery.retry_from_downloaded.profile;
        if ("retry_from_ready" in recovery) return recovery.retry_from_ready.profile;
      }
      return null;
    }
    default:
      return state.data.profile;
  }
}

/** Whether the machine is talking about the model the user has selected.
 * After a model switch the two diverge (machine still Ready with the old
 * profile), and the machine's phase must not be applied to the new one. */
export function machineMatchesSelection(
  state: AppStateMachine,
  selection: Pick<
    AppSettings,
    "transcription_engine_id" | "transcription_model_id" | "transcription_backend_id"
  >,
): boolean {
  const profile = machineProfile(state);
  if (!profile) return false;
  return (
    profile.engine_id === selection.transcription_engine_id
    && profile.model_id === selection.transcription_model_id
    && profile.backend_id === selection.transcription_backend_id
  );
}

function deriveModelOperationState(state: AppStateMachine): TranscriptionModelOperationState {
  switch (state.state) {
    case "downloading": return "downloading";
    case "loading": return "loading";
    case "unloading": return "unloading";
    default: return "idle";
  }
}

export function getAppState() {
  return {
    get appPermissions() { return appPermissions; },
    set appPermissions(v: PermissionStatus | null) { appPermissions = v; },

    get settingsOpen() { return settingsOpen; },
    set settingsOpen(v: boolean) { settingsOpen = v; },

    get settingsInitialAnchor() { return settingsInitialAnchor; },
    set settingsInitialAnchor(v: SettingsAnchor | null) { settingsInitialAnchor = v; },

    get permissionsPanelOpen() { return permissionsPanelOpen; },
    set permissionsPanelOpen(v: boolean) { permissionsPanelOpen = v; },

    get snippets() { return snippets; },
    set snippets(list: SnippetEntry[]) { snippets = list; },

    get currentMeetingId() { return currentMeetingId; },
    set currentMeetingId(id: string | null) { currentMeetingId = id; },

    get settings() { return settings; },
    set settings(s: AppSettings) { settings = s; },

    get selectedDevice() { return selectedDevice; },
    set selectedDevice(d: string) { selectedDevice = d; },

    get machineState() { return machineState; },
    set machineState(s: AppStateMachine) {
      const wasRecording = deriveRecordingMode(machineState) !== "idle";
      machineState = s;
      // A resumed meeting re-enters recording from a non-recording state, so
      // the counter restarts at zero for the new recording session.
      if (!wasRecording && deriveRecordingMode(s) !== "idle") {
        recordingStartedAtMs = Date.now();
      }
      // Sync runtime phase from machine when no model operation is in progress.
      // During download/load the phase is managed by runtime.ts callbacks.
      // Only when the machine is talking about the selected model: otherwise a
      // stray event would report the *old* model's phase (e.g. "ready") for a
      // model the user just picked and that still needs a download or a load.
      if (deriveModelOperationState(s) === "idle" && machineMatchesSelection(s, settings)) {
        transcriptionRuntimePhase = deriveRuntimePhase(s);
      }
      // Health snapshots only make sense while recording
      if (deriveRecordingMode(s) === "idle") {
        transcriptionHealth = null;
        systemAudioStatus = null;
        recordingStartedAtMs = null;
      }
    },

    get transcriptionHealth() { return transcriptionHealth; },
    set transcriptionHealth(h: TranscriptionHealth | null) { transcriptionHealth = h; },

    get recordingStartedAtMs() { return recordingStartedAtMs; },

    get systemAudioStatus() { return systemAudioStatus; },
    set systemAudioStatus(s: SystemAudioStatus | null) { systemAudioStatus = s; },
    get modifierTapStatus() { return modifierTapStatus; },
    set modifierTapStatus(s: ModifierTapStatus | null) { modifierTapStatus = s; },
    get nativeShortcuts() { return nativeShortcuts; },
    set nativeShortcuts(s: string[]) { nativeShortcuts = s; },
    get upcomingMeeting() { return upcomingMeeting; },
    set upcomingMeeting(u: UpcomingMeeting | null) { upcomingMeeting = u; },

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
