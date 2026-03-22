<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import Waveform from "./lib/components/Waveform.svelte";
  import TranscriptionView from "./lib/components/TranscriptionView.svelte";
  import MeetingView from "./lib/components/MeetingView.svelte";
  import MeetingHistoryView from "./lib/components/MeetingHistoryView.svelte";
  import SettingsView from "./lib/components/SettingsView.svelte";
  import { getAppState } from "./lib/stores/app.svelte";
  import type { Theme, View } from "./lib/types";

  const app = getAppState();

  let transcriptionView: TranscriptionView | undefined = $state();
  let meetingView: MeetingView | undefined = $state();

  let isAnyRecording = $derived(
    (transcriptionView?.getRecordingState?.() ?? false) ||
    (meetingView?.getRecordingState?.() ?? false)
  );

  let unlistenNav: (() => void) | null = null;

  onMount(() => {
    // Load saved settings from SQLite
    (async () => {
      try {
        const settings = await invoke<Record<string, unknown>>("get_settings");
        if (settings.theme) {
          const theme = settings.theme as Theme;
          app.theme = theme;
          app.settings = { ...app.settings, theme };
          applyTheme(theme);
        }
        if (settings.auto_paste !== null && settings.auto_paste !== undefined) {
          app.settings = { ...app.settings, auto_paste: settings.auto_paste as boolean };
        }
        if (settings.paste_delay_ms !== null && settings.paste_delay_ms !== undefined) {
          app.settings = { ...app.settings, paste_delay_ms: settings.paste_delay_ms as number };
        }
        if (settings.debug_transcription !== null && settings.debug_transcription !== undefined) {
          app.settings = { ...app.settings, debug_transcription: settings.debug_transcription as boolean };
        }
        if (settings.audio_device) {
          app.selectedDevice = settings.audio_device as string;
          await invoke("select_audio_device", { deviceName: app.selectedDevice });
        }
      } catch {
        // First run, no settings yet.
      }
    })();

    listen<string>("navigate", (event) => {
      const view = event.payload as View;
      if (["transcription", "meeting", "meeting-history", "settings"].includes(view)) {
        app.currentView = view;
      }
    }).then((fn) => {
      unlistenNav = fn;
    });

    return () => {
      unlistenNav?.();
    };
  });

  function applyTheme(theme: Theme) {
    if (theme === "dark" || (theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches)) {
      document.documentElement.classList.add("dark");
      document.documentElement.classList.remove("light");
    } else if (theme === "light") {
      document.documentElement.classList.remove("dark");
      document.documentElement.classList.add("light");
    } else {
      const isDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
      document.documentElement.classList.toggle("dark", isDark);
      document.documentElement.classList.toggle("light", !isDark);
    }
  }
</script>

<div class="flex h-screen overflow-hidden">
  <Sidebar />

  <div class="flex flex-1 flex-col min-w-0 overflow-hidden">
    <main class="flex-1 p-6 overflow-y-auto">
      {#if app.currentView === "transcription"}
        <TranscriptionView bind:this={transcriptionView} />
      {:else if app.currentView === "meeting"}
        <MeetingView bind:this={meetingView} />
      {:else if app.currentView === "meeting-history"}
        <MeetingHistoryView />
      {:else if app.currentView === "settings"}
        <SettingsView />
      {/if}
    </main>

    <Waveform active={isAnyRecording} />
  </div>
</div>
