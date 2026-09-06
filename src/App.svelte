<script lang="ts">
  import { ArrowLeft, Moon, SlidersHorizontal, Sun } from "@lucide/svelte";
  import { onMount } from "svelte";
  import { t } from "svelte-i18n";
  import HomeView from "./lib/components/HomeView.svelte";
  import SettingsView from "./lib/components/SettingsView.svelte";
  import StatusChip from "./lib/components/ui/StatusChip.svelte";
  import Toast from "./lib/components/ui/Toast.svelte";
  import { events } from "./lib/api/generated";
  import { saveSettings } from "./lib/api/settings";
  import { getTranscriptionCatalog, recoverState } from "./lib/api/transcription";
  import { getReleaseNotesForVersion } from "./lib/api/diagnostics";
  import { bootstrapAppState } from "./lib/bootstrap";
  import { findTranscriptionModel } from "./lib/features/transcription/catalog";
  import OnboardingView from "./lib/features/onboarding/OnboardingView.svelte";
  import WhatsNewDialog from "./lib/features/onboarding/WhatsNewDialog.svelte";
  import UpdateAvailableDialog from "./lib/features/onboarding/UpdateAvailableDialog.svelte";
  import {
    notifyMeetingAborted,
    notifyMeetingFinalized,
    notifyMeetingIdle,
    notifyMeetingStopRequested,
    notifySystemWokeUp,
  } from "./lib/features/meeting/controller.svelte";
  import {
    createTranscriptionController,
    notifyDictationAborted,
  } from "./lib/features/transcription/controller.svelte";
  import { getAppState } from "./lib/stores/app.svelte";
  import { applyTheme, errorMessage } from "./lib/utils";
  import { micToast, micToastCopy } from "./lib/features/audio/mic-toast.svelte";
  import { decideShowSetupWizard, readSetupFlags } from "./lib/features/onboarding/setup";
  import type { TranscriptionCatalog } from "./lib/types";

  const app = getAppState();
  // Mounted app-level so the global dictation shortcut works whatever view
  // (or the onboarding screen) is displayed.
  const transcription = createTranscriptionController();

  let unlistenNav: (() => void) | null = null;
  let unlistenUpdate: (() => void) | null = null;
  let unlistenState: (() => void) | null = null;
  let unlistenHealth: (() => void) | null = null;
  let unlistenPipelineError: (() => void) | null = null;

  let unlistenSystemAudio: (() => void) | null = null;
  let unlistenMeetingStop: (() => void) | null = null;
  let unlistenMeetingFinalized: (() => void) | null = null;
  let unlistenUpcomingMeeting: (() => void) | null = null;
  let unlistenMeetingIdle: (() => void) | null = null;
  let unlistenSystemWokeUp: (() => void) | null = null;
  let unlistenInputRoute: (() => void) | null = null;

  const healthDegraded = $derived(
    app.transcriptionHealth !== null && app.transcriptionHealth.status !== "healthy",
  );

  const machineError = $derived(
    app.machineState.state === "error" ? app.machineState.data : null,
  );
  const routeToast = $derived(micToast.current);
  const routeToastCopy = $derived(routeToast ? micToastCopy(routeToast, $t) : null);
  let isRecovering = $state(false);
  let whatsNew = $state<{ version: string; releaseNotes: string } | null>(null);
  let updateAvailable = $state<{
    version: string;
    releaseNotes: string | null;
    releaseUrl: string | null;
  } | null>(null);
  // Catalog is static; the label follows the *current* selection so the chip
  // updates as soon as the user switches model (no app restart needed).
  let transcriptionCatalog = $state<TranscriptionCatalog | null>(null);
  const modelLabel = $derived(
    app.settings.transcription_model_id
      ? findTranscriptionModel(
          transcriptionCatalog,
          app.settings.transcription_engine_id,
          app.settings.transcription_model_id,
        )?.label ?? ""
      : "",
  );

  const isLightTheme = $derived(
    app.settings.theme === "light" ||
      (app.settings.theme === "system" &&
        typeof window !== "undefined" &&
        !window.matchMedia("(prefers-color-scheme: dark)").matches),
  );

  function toggleTheme() {
    const next = isLightTheme ? "dark" : "light";
    app.settings.theme = next;
    applyTheme(next);
    saveSettings($state.snapshot(app.settings)).catch(() => {});
  }

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
        const result = await bootstrapAppState(app);
        whatsNew = result.whatsNew;
        if (result.whatsNew) {
          const targetVersion = result.whatsNew.version;
          void getReleaseNotesForVersion(targetVersion).then((notes) => {
            if (notes && whatsNew?.version === targetVersion) {
              whatsNew = { ...whatsNew, releaseNotes: notes };
            }
          });
        }
      } catch {
        // First run, no settings yet — still offer the setup wizard.
        if (decideShowSetupWizard("download_required", readSetupFlags())) {
          app.showOnboarding = true;
        }
      }
      cleanupTranscription = (await transcription.mount()) ?? (() => {});
      try {
        transcriptionCatalog = await getTranscriptionCatalog();
      } catch {
        // Header chip simply omits the model name.
      }
    })();

    events.updateAvailable.listen((event) => {
      // The backend emits at most once per launch per version, so this never
      // reopens a dialog the user has dismissed.
      updateAvailable = {
        version: event.payload.latest_version,
        releaseNotes: event.payload.release_notes,
        releaseUrl: event.payload.release_url,
      };
    }).then((fn) => {
      unlistenUpdate = fn;
    });

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

    events.meetingIdle.listen((event) => {
      notifyMeetingIdle(event.payload);
    }).then((fn) => {
      unlistenMeetingIdle = fn;
    });

    events.systemWokeUp.listen(() => {
      notifySystemWokeUp();
    }).then((fn) => {
      unlistenSystemWokeUp = fn;
    });

    events.inputRouteNotice.listen((event) => {
      micToast.show(event.payload);
    }).then((fn) => {
      unlistenInputRoute = fn;
    });

    // Belt and braces: the webview itself may have been suspended when the
    // backend's wake event fired (and so missed it), but visibility always
    // flips to visible when the window comes back, so recheck here too.
    // `take_sleep_paused_meeting` is idempotent (clears on read), so a
    // redundant call from both paths is harmless.
    const onVisibilityChange = () => {
      if (document.visibilityState === "visible") notifySystemWokeUp();
    };
    document.addEventListener("visibilitychange", onVisibilityChange);

    return () => {
      cleanupTranscription();
      unlistenNav?.();
      unlistenUpdate?.();
      unlistenState?.();
      unlistenHealth?.();
      unlistenPipelineError?.();
      unlistenSystemAudio?.();
      unlistenMeetingStop?.();
      unlistenMeetingFinalized?.();
      unlistenUpcomingMeeting?.();
      unlistenMeetingIdle?.();
      unlistenSystemWokeUp?.();
      unlistenInputRoute?.();
      document.removeEventListener("visibilitychange", onVisibilityChange);
    };
  });
  function dismissWhatsNew() {
    const version = whatsNew?.version;
    whatsNew = null;
    if (!version) return;
    const next = { ...app.settings, last_seen_version: version };
    app.settings = next;
    saveSettings(next).catch(() => {});
  }
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
    class="flex h-[52px] shrink-0 items-center gap-3 border-b border-ghost-border bg-white/[0.015] px-[18px] pl-[88px]"
  >
    <div class="flex items-center gap-[9px]" data-tauri-drag-region>
      <img
        src="/favicon.svg"
        alt=""
        class="h-[25px] w-[25px] rounded-[7px]"
        aria-hidden="true"
        data-tauri-drag-region
      />
      <span class="font-heading text-[14.5px] font-semibold" data-tauri-drag-region>Soufflé</span>
    </div>
    <span class="flex-1" data-tauri-drag-region></span>
    {#if app.recordingMode !== "idle"}
      <span
        class="inline-flex items-center gap-[7px] rounded-full bg-danger/14 px-[11px] py-[5px] text-xs font-semibold text-danger-soft outline-1 outline-danger/30"
      >
        <span class="recording-dot"></span>
        {$t("meeting_header.recording_badge")}
      </span>
    {:else}
      <StatusChip
        phase={app.transcriptionRuntimePhase}
        operationState={app.transcriptionModelOperationState}
        downloadedBytes={app.downloadedBytes}
        downloadTotalBytes={app.downloadTotalBytes}
        {modelLabel}
      />
    {/if}
    <button
      onclick={toggleTheme}
      class="flex h-8 w-8 cursor-pointer items-center justify-center rounded-[9px] text-text-muted transition-colors hover:bg-surface-2 hover:text-text-primary"
      aria-label={$t("ui.toggle_theme")}
      title={$t("ui.toggle_theme")}
    >
      {#if isLightTheme}
        <Moon size={16} aria-hidden="true" />
      {:else}
        <Sun size={17} aria-hidden="true" />
      {/if}
    </button>
    <button
      onclick={() => (app.settingsOpen = !app.settingsOpen)}
      class="flex h-8 w-8 cursor-pointer items-center justify-center rounded-[9px] text-text-muted transition-colors hover:bg-surface-2 hover:text-text-primary"
      aria-label={$t("settings.title")}
      title="⌘,"
    >
      <SlidersHorizontal size={18} aria-hidden="true" />
    </button>
  </header>

  <div class="flex flex-1 flex-col min-w-0 overflow-hidden">
    <main class="flex-1 overflow-y-auto px-7 pb-[34px] pt-[26px]">
      {#if app.settingsOpen}
        <div class="mx-auto flex w-full max-w-[720px] flex-col gap-[22px]">
          <button
            onclick={() => (app.settingsOpen = false)}
            class="btn btn-ghost -ml-1.5 gap-1.5 self-start px-2.5 py-1 text-[13px]"
          >
            <ArrowLeft size={16} aria-hidden="true" />
            {$t("settings.done")}
          </button>
          <h1 class="text-[23px] font-bold">{$t("settings.title")}</h1>
          <SettingsView />
        </div>
      {:else}
        <HomeView />
      {/if}
    </main>

    {#if machineError}
      <div
        class="flex items-center justify-between gap-3 border-t border-danger/30 bg-danger/10 px-4 py-2 text-sm text-danger-soft"
        role="alert"
      >
        <span class="truncate">
          {$t("pipeline.error")}: {machineError.message}
        </span>
        <button
          class="shrink-0 rounded-md border border-danger/40 px-2 py-0.5 text-xs hover:bg-danger/20 disabled:opacity-50"
          disabled={isRecovering}
          onclick={recoverFromError}
        >
          {$t("pipeline.recover")}
        </button>
      </div>
    {:else if app.pipelineError}
      <div
        class="flex items-center justify-between gap-3 border-t border-danger/30 bg-danger/10 px-4 py-2 text-sm text-danger-soft"
        role="alert"
      >
        <span class="truncate">
          {$t("pipeline.error")}: {app.pipelineError.message}
        </span>
        <button
          class="shrink-0 rounded-md px-2 py-0.5 text-xs hover:bg-danger/20"
          onclick={() => (app.pipelineError = null)}
        >
          {$t("pipeline.dismiss")}
        </button>
      </div>
    {:else if healthDegraded}
      <div
        class="border-t border-warning/30 bg-warning/10 px-4 py-2 text-sm text-warning"
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

{#if whatsNew && !app.showOnboarding}
  <WhatsNewDialog
    version={whatsNew.version}
    releaseNotes={whatsNew.releaseNotes}
    onDismiss={dismissWhatsNew}
  />
{/if}

{#if updateAvailable && !app.showOnboarding}
  <UpdateAvailableDialog
    version={updateAvailable.version}
    releaseNotes={updateAvailable.releaseNotes}
    releaseUrl={updateAvailable.releaseUrl}
    onDismiss={() => (updateAvailable = null)}
  />
{/if}

{#if !app.showOnboarding && routeToast && routeToastCopy}
  <div class="pointer-events-none fixed inset-x-0 bottom-6 z-40 flex justify-center px-4">
    <div class="pointer-events-auto">
      <Toast
        title={routeToastCopy.title}
        detail={routeToastCopy.detail}
        hint={routeToastCopy.hint}
        onDismiss={() => micToast.dismiss()}
      />
    </div>
  </div>
{/if}
