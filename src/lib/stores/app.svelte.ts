import type { View, Theme, AppSettings } from "../types";

// Current view
let currentView = $state<View>("transcription");

// Current meeting ID (when viewing a specific meeting)
let currentMeetingId = $state<string | null>(null);

// Theme
let theme = $state<Theme>("dark");

// Selected audio device (persisted across tab switches)
let selectedDevice = $state("");

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

    get theme() { return theme; },
    set theme(t: Theme) { theme = t; },

    get settings() { return settings; },
    set settings(s: AppSettings) { settings = s; },

    get selectedDevice() { return selectedDevice; },
    set selectedDevice(d: string) { selectedDevice = d; },

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
