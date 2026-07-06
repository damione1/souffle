<script lang="ts">
  import { Square } from "@lucide/svelte";
  import { onMount } from "svelte";
  import { t } from "svelte-i18n";
  import { events } from "../api/generated";
  import { getMachineState } from "../api/transcription";
  import Waveform from "../components/Waveform.svelte";
  import type { AppStateMachine } from "../types";

  let machineState = $state<AppStateMachine>({ state: "idle" });
  let unlistenState: (() => void) | null = null;

  const mode = $derived.by((): "dictation" | "meeting" | null => {
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

  function stop() {
    if (mode === "meeting") {
      void events.meetingStopRequested.emit();
    } else {
      void events.shortcutToggle.emit();
    }
  }

  onMount(() => {
    document.body.classList.add("pill-body");

    void getMachineState().then((state) => {
      machineState = state;
    });
    void events.stateChanged.listen((event) => {
      machineState = event.payload;
    }).then((fn) => {
      unlistenState = fn;
    });

    return () => unlistenState?.();
  });
</script>

<div
  data-tauri-drag-region
  class="flex h-full w-full items-center gap-3 rounded-full border border-white/10 bg-black/75 px-4 shadow-lg backdrop-blur-md"
>
  <span class="recording-dot shrink-0" aria-hidden="true"></span>
  <span class="shrink-0 text-xs font-medium text-white/90" data-tauri-drag-region>
    {mode === "meeting" ? $t("pill.meeting") : $t("pill.dictation")}
  </span>
  <div class="min-w-0 flex-1">
    <Waveform active variant="pill" />
  </div>
  <button
    onclick={stop}
    class="flex h-7 w-7 shrink-0 cursor-pointer items-center justify-center rounded-full bg-red-500/90 text-white transition-colors hover:bg-red-500"
    aria-label={$t("pill.stop")}
  >
    <Square size={12} fill="currentColor" aria-hidden="true" />
  </button>
</div>
