<script lang="ts">
  import { onMount } from "svelte";
  import { t } from "svelte-i18n";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import TranscriptionView from "./lib/components/TranscriptionView.svelte";
  import MeetingsView from "./lib/components/MeetingsView.svelte";
  import SettingsView from "./lib/components/SettingsView.svelte";
  import { events } from "./lib/api/generated";
  import { recoverState } from "./lib/api/transcription";
  import { bootstrapAppState } from "./lib/bootstrap";
  import OnboardingView from "./lib/features/onboarding/OnboardingView.svelte";
  import {
    notifyMeetingAborted,
    notifyMeetingStopRequested,
  } from "./lib/features/meeting/controller.svelte";
  import {
    createTranscriptionController,
    notifyDictationAborted,
  } from "./lib/features/transcription/controller.svelte";
  import { getAppState } from "./lib/stores/app.svelte";
  import { errorMessage } from "./lib/utils";

  const app = getAppState();
  // Mounted app-level so the global dictation shortcut works whatever view
  // (or the onboarding screen) is displayed.
  const transcription = createTranscriptionController();

  let unlistenNav: (() => void) | null = null;
  let unlistenState: (() => void) | null = null;
  let unlistenHealth: (() => void) | null = null;
  let unlistenPipelineError: (() => void) | null = null;

  let unlistenSystemAudio: (() => void) | null = null;
  let unlistenMeetingStop: (() => void) | null = null;

  const healthDegraded = $derived(
    app.transcriptionHealth !== null && app.transcriptionHealth.status !== "healthy",
  );

  const machineError = $derived(
    app.machineState.state === "error" ? app.machineState.data : null,
  );
  let isRecovering = $state(false);

  async function recoverFromError() {
    isRecovering = true;
    try {
      app.machineState = await recoverState();
      app.pipelineError = null;
    } catch (e) {
      console.warn("State recovery failed:", errorMessage(e));
    } finally {
      isRecovering = false;
    }
  }

  function wasRecording(state: typeof app.machineState): "dictation" | "meeting" | null {
    switch (state.state) {
      case "recording_dictation":
        return "dictation";
      case "recording_meeting":
        return "meeting";
      case "stopping":
        return typeof state.data.was_recording === "object" ? "meeting" : "dictation";
      default:
        return null;
    }
  }

  onMount(() => {
    let cleanupTranscription = () => {};
    (async () => {
      try {
        await bootstrapAppState(app);
      } catch {
        // First run, no settings yet.
      }
      cleanupTranscription = (await transcription.mount()) ?? (() => {});
    })();

    events.navigate.listen((event) => {
      app.currentView = event.payload;
    }).then((fn) => {
      unlistenNav = fn;
    });

    events.stateChanged.listen((event) => {
      // Detect a backend-initiated session abort (recording → error) so the
      // recorder controllers can reset their local state.
      const aborted = event.payload.state === "error" ? wasRecording(app.machineState) : null;
      app.machineState = event.payload;
      if (aborted === "dictation") notifyDictationAborted();
      else if (aborted === "meeting") notifyMeetingAborted();
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

    events.systemAudioStatus.listen((event) => {
      app.systemAudioStatus = event.payload;
    }).then((fn) => {
      unlistenSystemAudio = fn;
    });

    events.meetingStopRequested.listen(() => {
      notifyMeetingStopRequested();
    }).then((fn) => {
      unlistenMeetingStop = fn;
    });

    return () => {
      cleanupTranscription();
      unlistenNav?.();
      unlistenState?.();
      unlistenHealth?.();
      unlistenPipelineError?.();
      unlistenSystemAudio?.();
      unlistenMeetingStop?.();
    };
  });
</script>

{#if app.showOnboarding}
  <OnboardingView />
{:else}
<div class="flex h-screen overflow-hidden">
  <Sidebar />

  <div class="flex flex-1 flex-col min-w-0 overflow-hidden">
    <main class="flex-1 p-6 overflow-y-auto">
      {#if app.currentView === "transcription"}
        <TranscriptionView />
      {:else if app.currentView === "meetings"}
        <MeetingsView />
      {:else if app.currentView === "settings"}
        <SettingsView />
      {/if}
    </main>

    {#if machineError}
      <div
        class="flex items-center justify-between gap-3 border-t border-red-500/30 bg-red-500/10 px-4 py-2 text-sm text-red-400"
        role="alert"
      >
        <span class="truncate">
          {$t("pipeline.error")}: {machineError.message}
        </span>
        <button
          class="shrink-0 rounded border border-red-500/40 px-2 py-0.5 text-xs hover:bg-red-500/20 disabled:opacity-50"
          disabled={isRecovering}
          onclick={recoverFromError}
        >
          {$t("pipeline.recover")}
        </button>
      </div>
    {:else if app.pipelineError}
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
  </div>
</div>
{/if}
