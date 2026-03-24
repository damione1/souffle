<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import Waveform from "./lib/components/Waveform.svelte";
  import TranscriptionView from "./lib/components/TranscriptionView.svelte";
  import MeetingView from "./lib/components/MeetingView.svelte";
  import MeetingHistoryView from "./lib/components/MeetingHistoryView.svelte";
  import SettingsView from "./lib/components/SettingsView.svelte";
  import { bootstrapAppState } from "./lib/bootstrap";
  import { getAppState } from "./lib/stores/app.svelte";
  import type { View } from "./lib/types";

  const app = getAppState();

  let unlistenNav: (() => void) | null = null;

  onMount(() => {
    (async () => {
      try {
        await bootstrapAppState(app);
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
</script>

<div class="flex h-screen overflow-hidden">
  <Sidebar />

  <div class="flex flex-1 flex-col min-w-0 overflow-hidden">
    <main class="flex-1 p-6 overflow-y-auto">
      {#if app.currentView === "transcription"}
        <TranscriptionView />
      {:else if app.currentView === "meeting"}
        <MeetingView />
      {:else if app.currentView === "meeting-history"}
        <MeetingHistoryView />
      {:else if app.currentView === "settings"}
        <SettingsView />
      {/if}
    </main>

    <Waveform active={app.isRecording} />
  </div>
</div>
