<script lang="ts">
  import { onMount } from "svelte";
  import { t } from "svelte-i18n";
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
  let unlistenHealth: (() => void) | null = null;
  let unlistenPipelineError: (() => void) | null = null;

  const healthDegraded = $derived(
    app.transcriptionHealth !== null && app.transcriptionHealth.status !== "healthy",
  );

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

    events.transcriptionHealth.listen((event) => {
      app.transcriptionHealth = event.payload;
    }).then((fn) => {
      unlistenHealth = fn;
    });

    events.pipelineError.listen((event) => {
      app.pipelineError = event.payload;
    }).then((fn) => {
      unlistenPipelineError = fn;
    });

    return () => {
      unlistenNav?.();
      unlistenState?.();
      unlistenHealth?.();
      unlistenPipelineError?.();
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

    {#if app.pipelineError}
      <div
        class="flex items-center justify-between gap-3 border-t border-red-500/30 bg-red-500/10 px-4 py-2 text-sm text-red-400"
        role="alert"
      >
        <span class="truncate">
          {$t("pipeline.error")}: {app.pipelineError.message}
        </span>
        <button
          class="shrink-0 rounded px-2 py-0.5 text-xs hover:bg-red-500/20"
          onclick={() => (app.pipelineError = null)}
        >
          {$t("pipeline.dismiss")}
        </button>
      </div>
    {:else if healthDegraded}
      <div
        class="border-t border-amber-500/30 bg-amber-500/10 px-4 py-2 text-sm text-amber-400"
        role="status"
      >
        {app.transcriptionHealth?.status === "stalled"
          ? $t("pipeline.stalled")
          : $t("pipeline.lagging")}
      </div>
    {/if}

    <Waveform active={app.isRecording} />
  </div>
</div>
