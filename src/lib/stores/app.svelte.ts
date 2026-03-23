import type { View, AppSettings } from "../types";

export type RecordingMode = "idle" | "dictation" | "meeting";

// Current view
let currentView = $state<View>("transcription");

// Current meeting ID (when viewing a specific meeting)
let currentMeetingId = $state<string | null>(null);

// Selected audio device (persisted across tab switches)
let selectedDevice = $state("");

// Recording state — shared across views
let isRecording = $state(false);
let recordingMode = $state<RecordingMode>("idle");

// Settings with defaults
let settings = $state<AppSettings>({
  theme: "dark",
  auto_paste: false,
  paste_delay_ms: 100,
  ollama_url: "http://localhost:11434",
  ollama_model: "",
  debug_transcription: false,
});

export function getAppState() {
  return {
    get currentView() { return currentView; },
    set currentView(v: View) { currentView = v; },

    get currentMeetingId() { return currentMeetingId; },
    set currentMeetingId(id: string | null) { currentMeetingId = id; },

    get settings() { return settings; },
    set settings(s: AppSettings) { settings = s; },

    get selectedDevice() { return selectedDevice; },
    set selectedDevice(d: string) { selectedDevice = d; },

    get isRecording() { return isRecording; },
    set isRecording(v: boolean) { isRecording = v; },

    get recordingMode() { return recordingMode; },
    set recordingMode(m: RecordingMode) { recordingMode = m; },

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
