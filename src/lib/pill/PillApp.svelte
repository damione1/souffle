<script lang="ts">
  import { LogicalSize } from "@tauri-apps/api/dpi";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { Square } from "@lucide/svelte";
  import { onMount } from "svelte";
  import { t } from "svelte-i18n";
  import { events } from "../api/generated";
  import { getMachineState, pillRecenter } from "../api/transcription";
  import Spinner from "../components/ui/Spinner.svelte";
  import Waveform from "../components/Waveform.svelte";
  import type { AppStateMachine, PillHoldKind } from "../types";

  // Compact state (idle row only): must match the "pill" window's initial
  // size in tauri.conf.json.
  const PILL_WIDTH = 280;
  const BASE_HEIGHT = 64;
  // Expanded state (live-text preview showing): wide immediately; height is
  // measured from the rounded container's natural content instead, so it
  // grows line by line as the live-text tail fills in.
  const EXPANDED_WIDTH = 440;
  // Hard safety net only — the rounded container's real ceiling comes from
  // `line-clamp-5` on the live-text paragraph (header row + 5 lines of
  // 13px/leading-snug text + paddings), which already bounds the measured
  // height well below this.
  const MAX_HEIGHT = 200;

  let machineState = $state<AppStateMachine>({ state: "idle" });
  let holdKind = $state<PillHoldKind | null>(null);
  let liveText = $state("");
  let measuredHeight = $state(BASE_HEIGHT);
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

  // Imperative resize bookkeeping, deliberately not reactive state.
  let sessionMaxHeight = BASE_HEIGHT;
  let appliedWidth = 0;
  let appliedHeight = 0;

  $effect(() => {
    const targetWidth = showLiveText ? EXPANDED_WIDTH : PILL_WIDTH;
    let targetHeight = BASE_HEIGHT;
    if (showLiveText) {
      // measuredHeight tracks the auto-height container, so it only moves
      // when the wrapped line count changes. Monotonic within a session: a
      // momentarily shorter tail must not shrink the pill mid-dictation.
      const clamped = Math.min(Math.max(measuredHeight, BASE_HEIGHT), MAX_HEIGHT);
      sessionMaxHeight = Math.max(sessionMaxHeight, clamped);
      targetHeight = sessionMaxHeight;
    } else {
      sessionMaxHeight = BASE_HEIGHT;
    }
    // Skip no-op resizes so the post-resize re-measure settles instead of
    // looping.
    if (Math.abs(targetWidth - appliedWidth) <= 1 && Math.abs(targetHeight - appliedHeight) <= 1) {
      return;
    }
    appliedWidth = targetWidth;
    appliedHeight = targetHeight;
    // setSize keeps the window's top-left corner fixed, so a width change
    // drifts the pill off-center. Recenter once the resize lands.
    void getCurrentWindow()
      .setSize(new LogicalSize(targetWidth, targetHeight))
      .then(() => pillRecenter())
      .catch((e) => console.warn("Pill resize/recenter failed:", e));
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

<!-- Auto-height and top-anchored: the window height chases this container's
     measured height, so during the brief window/content mismatch nothing sits
     off-center or leaves a dark band below the rounded shape. -->
<div
  data-tauri-drag-region
  bind:offsetHeight={measuredHeight}
  class="flex w-full flex-col gap-1 rounded-[28px] border border-white/10 bg-black/75 px-4 py-2.5 shadow-lg backdrop-blur-md"
>
  <!-- h-[42px] keeps the compact pill's natural height at exactly
       BASE_HEIGHT (42 + 2x10 padding + 2x1 border = 64). -->
  <div class="flex h-[42px] shrink-0 items-center gap-3">
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
      class="line-clamp-5 border-t border-accent/15 pt-2 pb-1 pl-[19px] text-[13px] leading-snug text-white/70"
      data-tauri-drag-region
    >
      {liveText}
    </p>
  {/if}
</div>
