import type { View, Theme, AppSettings } from "../types";

// Current view
let currentView = $state<View>("dictation");

// Theme
let theme = $state<Theme>("dark");

// Settings with defaults
let settings = $state<AppSettings>({
  theme: "dark",
  auto_paste: false,
  paste_delay_ms: 100,
  ollama_url: "http://localhost:11434",
  ollama_model: "",
});

export function getAppState() {
  return {
    get currentView() { return currentView; },
    set currentView(v: View) { currentView = v; },

    get theme() { return theme; },
    set theme(t: Theme) { theme = t; },

    get settings() { return settings; },
    set settings(s: AppSettings) { settings = s; },
  };
}
