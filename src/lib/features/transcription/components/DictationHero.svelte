<script lang="ts">
  import { Mic } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import Waveform from "../../../components/Waveform.svelte";
  import type { TranscriptionRuntimePhase } from "../../../types";

  type HeroPhase = "locked_by_meeting" | "starting" | "recording" | "preparing" | "ready";

  let {
    isStartingRecording,
    isRecording,
    lockedByMeeting,
    runtimePhase,
    onToggleRecording,
  }: {
    isStartingRecording: boolean;
    isRecording: boolean;
    lockedByMeeting: boolean;
    runtimePhase: TranscriptionRuntimePhase;
    onToggleRecording: () => void | Promise<void>;
  } = $props();

  let phase = $derived.by((): HeroPhase => {
    if (lockedByMeeting) return "locked_by_meeting";
    if (isStartingRecording) return "starting";
    if (isRecording) return "recording";
    if (runtimePhase !== "ready") return "preparing";
    return "ready";
  });

  const headingKeys: Record<HeroPhase, string> = {
    locked_by_meeting: "recorder.heading_locked",
    starting: "recorder.heading_starting",
    recording: "recorder.heading_recording",
    preparing: "recorder.heading_default",
    ready: "recorder.heading_default",
  };

  const descriptionKeys: Record<HeroPhase, string> = {
    locked_by_meeting: "recorder.desc_locked",
    starting: "recorder.desc_starting",
    recording: "recorder.desc_recording",
    preparing: "recorder.desc_preparing",
    ready: "recorder.desc_ready",
  };
</script>

<section class="surface-card flex flex-col items-center gap-4 text-center">
  <h3>{$t(headingKeys[phase])}</h3>
  <p class="text-text-secondary text-sm">{$t(descriptionKeys[phase])}</p>

  <button
    onclick={onToggleRecording}
    disabled={phase === "starting" || phase === "locked_by_meeting" || phase === "preparing"}
    aria-label={phase === "recording"
      ? $t("recorder.stop_recording_aria")
      : $t("recorder.start_recording_aria")}
    class="record-button"
    class:is-starting={phase === "starting"}
    class:is-recording={phase === "recording"}
  >
    <Mic size={40} aria-hidden="true" />
  </button>

  <div class="w-full max-w-xs">
    <Waveform active={isRecording} variant="inline" />
  </div>
</section>
