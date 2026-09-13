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

// No default written here: `AppSettings::default()` in src-tauri/src/settings.rs
// is the only declaration, and it reaches the webview through
// `get_default_settings`. `main.ts` fills this in before mounting, so the
// getter below never sees null in the running app.
let settings = $state<AppSettings | null>(null);

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

    get settings(): AppSettings {
      if (settings === null) {
        throw new Error(
          "app settings read before bootstrap: main.ts awaits loadSettingsOrDefaults() before mounting",
        );
      }
      return settings;
    },
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
      if (
        settings !== null
        && deriveModelOperationState(s) === "idle"
        && machineMatchesSelection(s, settings)
      ) {
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
