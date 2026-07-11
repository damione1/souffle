<script lang="ts">
  import { LogicalSize } from "@tauri-apps/api/dpi";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { Square } from "@lucide/svelte";
  import { onMount } from "svelte";
  import { t } from "svelte-i18n";
  import { events } from "../api/generated";
  import { getMachineState } from "../api/transcription";
  import Spinner from "../components/ui/Spinner.svelte";
  import Waveform from "../components/Waveform.svelte";
  import type { AppStateMachine, PillHoldKind } from "../types";

  // Must match the "pill" window's initial size in tauri.conf.json.
  const PILL_WIDTH = 280;
  const BASE_HEIGHT = 64;
  // Extra room for the 2-3 line live-text preview below the main row.
  const EXPANDED_HEIGHT = 108;

  let machineState = $state<AppStateMachine>({ state: "idle" });
  let holdKind = $state<PillHoldKind | null>(null);
  let liveText = $state("");
  let unlistenState: (() => void) | null = null;
  let unlistenLiveText: (() => void) | null = null;
  let unlistenHold: (() => void) | null = null;

  const recordingMode = $derived.by((): "dictation" | "meeting" | null => {
    switch (machineState.state) {
      case "recording_dictation":
        return "dictation";
      case "recording_meeting":
        return "meeting";
      case "stopping":
        return typeof machineState.data.was_recording === "object" ? "meeting" : "dictation";
      default:
        return null;
    }
  });

  // A hold outlives the recording state (e.g. dictation polish still running
  // after the state machine already left recording_dictation), so it takes
  // priority over whatever the state machine currently reports.
  const displayMode = $derived.by((): "dictation" | "meeting" | "polishing" | null => {
    if (holdKind === "polishing") return "polishing";
    return recordingMode;
  });

  const showLiveText = $derived(displayMode === "dictation" && liveText.trim().length > 0);

  function stop() {
    if (recordingMode === "meeting") {
      void events.meetingStopRequested.emit();
    } else {
      void events.shortcutToggle.emit();
    }
  }

  $effect(() => {
    const targetHeight = showLiveText ? EXPANDED_HEIGHT : BASE_HEIGHT;
    void getCurrentWindow().setSize(new LogicalSize(PILL_WIDTH, targetHeight));
  });

  onMount(() => {
    document.body.classList.add("pill-body");

    void getMachineState().then((state) => {
      machineState = state;
    });

    void events.stateChanged.listen((event) => {
      const wasRecording =
        machineState.state === "recording_dictation" || machineState.state === "recording_meeting";
      machineState = event.payload;
      const isRecording =
        machineState.state === "recording_dictation" || machineState.state === "recording_meeting";
      // A fresh session starting makes any leftover live text from a
      // previous (e.g. interrupted) session stale — belt and braces
      // alongside the backend's own clear-on-stop emission.
      if (isRecording && !wasRecording) {
        liveText = "";
      }
    }).then((fn) => {
      unlistenState = fn;
    });

    void events.dictationLiveText.listen((event) => {
      liveText = event.payload.text;
    }).then((fn) => {
      unlistenLiveText = fn;
    });

    void events.pillHoldChanged.listen((event) => {
      holdKind = event.payload.kind;
    }).then((fn) => {
      unlistenHold = fn;
    });

    return () => {
      unlistenState?.();
      unlistenLiveText?.();
      unlistenHold?.();
    };
  });
</script>

<div
  data-tauri-drag-region
  class="flex h-full w-full flex-col justify-center gap-1.5 rounded-[28px] border border-white/10 bg-black/75 px-4 py-2.5 shadow-lg backdrop-blur-md"
>
  <div class="flex shrink-0 items-center gap-3">
    <span class="recording-dot shrink-0" aria-hidden="true"></span>
    <span class="shrink-0 text-xs font-medium text-white/90" data-tauri-drag-region>
      {#if displayMode === "polishing"}
        {$t("pill.polishing")}
      {:else}
        {displayMode === "meeting" ? $t("pill.meeting") : $t("pill.dictation")}
      {/if}
    </span>
    <div class="min-w-0 flex-1">
      {#if displayMode === "polishing"}
        <div class="flex items-center justify-center text-accent">
          <Spinner />
        </div>
      {:else}
        <Waveform active variant="pill" />
      {/if}
    </div>
    {#if displayMode !== "polishing"}
      <button
        onclick={stop}
        class="flex h-7 w-7 shrink-0 cursor-pointer items-center justify-center rounded-full bg-red-500/90 text-white transition-colors hover:bg-red-500"
        aria-label={$t("pill.stop")}
      >
        <Square size={12} fill="currentColor" aria-hidden="true" />
      </button>
    {/if}
  </div>
  {#if showLiveText}
    <p
      class="line-clamp-3 pl-[19px] text-[11px] leading-snug text-white/60"
      data-tauri-drag-region
    >
      {liveText}
    </p>
  {/if}
</div>
