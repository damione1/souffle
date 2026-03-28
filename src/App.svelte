<script lang="ts">
  import { onMount } from "svelte";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import Waveform from "./lib/components/Waveform.svelte";
  import TranscriptionView from "./lib/components/TranscriptionView.svelte";
  import MeetingView from "./lib/components/MeetingView.svelte";
  import MeetingHistoryView from "./lib/components/MeetingHistoryView.svelte";
  import SettingsView from "./lib/components/SettingsView.svelte";
  import { events } from "./lib/api/generated";
  import { bootstrapAppState } from "./lib/bootstrap";
  import { getAppState } from "./lib/stores/app.svelte";

  const app = getAppState();

  let unlistenNav: (() => void) | null = null;
  let unlistenState: (() => void) | null = null;

  onMount(() => {
    (async () => {
      try {
        await bootstrapAppState(app);
      } catch {
        // First run, no settings yet.
      }
    })();

    events.navigate.listen((event) => {
      app.currentView = event.payload;
    }).then((fn) => {
      unlistenNav = fn;
    });

    events.stateChanged.listen((event) => {
      app.machineState = event.payload;
    }).then((fn) => {
      unlistenState = fn;
    });

    return () => {
      unlistenNav?.();
      unlistenState?.();
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
