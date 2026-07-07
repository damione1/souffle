<script lang="ts">
  import { Settings as SettingsIcon } from "@lucide/svelte";
  import { onMount } from "svelte";
  import { t } from "svelte-i18n";
  import HomeView from "./lib/components/HomeView.svelte";
  import SettingsView from "./lib/components/SettingsView.svelte";
  import Sheet from "./lib/components/ui/Sheet.svelte";
  import StatusChip from "./lib/components/ui/StatusChip.svelte";
  import { events } from "./lib/api/generated";
  import { recoverState } from "./lib/api/transcription";
  import { bootstrapAppState } from "./lib/bootstrap";
  import OnboardingView from "./lib/features/onboarding/OnboardingView.svelte";
  import PermissionsOnboarding from "./lib/features/onboarding/PermissionsOnboarding.svelte";
  import {
    notifyMeetingAborted,
    notifyMeetingFinalized,
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
  let unlistenMeetingFinalized: (() => void) | null = null;
  let unlistenUpcomingMeeting: (() => void) | null = null;

  const healthDegraded = $derived(
    app.transcriptionHealth !== null && app.transcriptionHealth.status !== "healthy",
  );

  const machineError = $derived(
    app.machineState.state === "error" ? app.machineState.data : null,
  );
  let isRecovering = $state(false);
  let showPermissions = $state(false);

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

    // First-run permissions walkthrough (mic, system audio, accessibility) so
    // the user grants everything up front instead of hitting prompts piecemeal.
    try {
      showPermissions = localStorage.getItem("permissionsOnboarded") !== "1";
    } catch {
      showPermissions = false;
    }

    events.navigate.listen((event) => {
      if (event.payload === "settings") {
        app.settingsOpen = true;
      } else {
        app.settingsOpen = false;
        app.currentMeetingId = null;
      }
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

    events.meetingFinalized.listen((event) => {
      notifyMeetingFinalized(event.payload.id);
    }).then((fn) => {
      unlistenMeetingFinalized = fn;
    });

    events.upcomingMeeting.listen((event) => {
      app.upcomingMeeting = event.payload;
    }).then((fn) => {
      unlistenUpcomingMeeting = fn;
    });

    return () => {
      cleanupTranscription();
      unlistenNav?.();
      unlistenState?.();
      unlistenHealth?.();
      unlistenPipelineError?.();
      unlistenSystemAudio?.();
      unlistenMeetingStop?.();
      unlistenMeetingFinalized?.();
      unlistenUpcomingMeeting?.();
    };
  });
</script>

<svelte:window
  onkeydown={(event) => {
    if (event.key === "," && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      app.settingsOpen = !app.settingsOpen;
    }
  }}
/>

{#if app.showOnboarding}
  <OnboardingView />
{:else}
<div class="flex h-screen flex-col overflow-hidden">
  <header
    data-tauri-drag-region
    class="flex shrink-0 items-center gap-3 px-5 pt-3 pb-2 pl-[88px]"
  >
    <img src="/favicon.svg" alt="" class="h-6 w-6 rounded-md" aria-hidden="true" data-tauri-drag-region />
    <span class="font-heading text-sm font-bold tracking-tight" data-tauri-drag-region>Soufflé</span>
    <span class="flex-1" data-tauri-drag-region></span>
    <StatusChip
      phase={app.transcriptionRuntimePhase}
      operationState={app.transcriptionModelOperationState}
      downloadedBytes={app.downloadedBytes}
      downloadTotalBytes={app.downloadTotalBytes}
    />
    <button
      onclick={() => (app.settingsOpen = true)}
      class="cursor-pointer rounded-lg p-1.5 text-text-muted transition-colors hover:bg-surface-2 hover:text-text-primary"
      aria-label={$t("settings.title")}
      title="⌘,"
    >
      <SettingsIcon size={17} aria-hidden="true" />
    </button>
  </header>

  <div class="flex flex-1 flex-col min-w-0 overflow-hidden">
    <main class="flex-1 overflow-y-auto px-6 pb-6 pt-2">
      <HomeView />
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

{#if app.settingsOpen}
  <Sheet title={$t("settings.title")} onClose={() => (app.settingsOpen = false)}>
    <SettingsView />
  </Sheet>
{/if}
{/if}

{#if showPermissions}
  <PermissionsOnboarding onClose={() => (showPermissions = false)} />
{/if}
