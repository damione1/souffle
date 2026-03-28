<script lang="ts">
  import { t } from "svelte-i18n";
  import CopyButton from "../../../components/ui/CopyButton.svelte";
  import EmptyState from "../../../components/ui/EmptyState.svelte";

  type TranscriptPhase = "has_text" | "starting" | "recording" | "idle";

  let {
    transcript,
    isStartingRecording,
    isRecording,
  }: {
    transcript: string;
    isStartingRecording: boolean;
    isRecording: boolean;
  } = $props();

  let phase = $derived.by((): TranscriptPhase => {
    if (transcript) return "has_text";
    if (isStartingRecording) return "starting";
    if (isRecording) return "recording";
    return "idle";
  });

  const emptyTitleKeys: Record<Exclude<TranscriptPhase, "has_text">, string> = {
    starting: "transcript.empty_starting_title",
    recording: "transcript.empty_recording_title",
    idle: "transcript.empty_idle_title",
  };

  const emptyMessageKeys: Record<Exclude<TranscriptPhase, "has_text">, string> = {
    starting: "transcript.empty_starting_msg",
    recording: "transcript.empty_recording_msg",
    idle: "transcript.empty_idle_msg",
  };
</script>

<section class="surface-card flex flex-col gap-3">
  <div class="flex items-center justify-between gap-4 flex-wrap">
    <h3>{$t("transcript.title")}</h3>
    {#if phase === "has_text"}
      <CopyButton text={transcript} />
    {/if}
  </div>

  {#if phase === "has_text"}
    <div class="p-3 bg-surface-1 rounded-default outline-1 outline-ghost-border text-text-secondary whitespace-pre-wrap min-h-40 max-h-[360px] overflow-y-auto text-sm leading-relaxed">{transcript}</div>
  {:else}
    <EmptyState title={$t(emptyTitleKeys[phase])} message={$t(emptyMessageKeys[phase])} />
  {/if}
</section>
