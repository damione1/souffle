<script lang="ts">
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

  const emptyTitles: Record<Exclude<TranscriptPhase, "has_text">, string> = {
    starting: "Warming up...",
    recording: "Listening for speech",
    idle: "No transcript yet",
  };

  const emptyMessages: Record<Exclude<TranscriptPhase, "has_text">, string> = {
    starting: "Text will appear as you speak.",
    recording: "Text will appear as you speak.",
    idle: "Press the mic button to start.",
  };
</script>

<section class="surface-card flex flex-col gap-3">
  <div class="flex items-center justify-between gap-4 flex-wrap">
    <h3>Transcript</h3>
    {#if phase === "has_text"}
      <CopyButton text={transcript} />
    {/if}
  </div>

  {#if phase === "has_text"}
    <div class="p-3 bg-surface-1 rounded-default outline-1 outline-ghost-border text-text-secondary whitespace-pre-wrap min-h-40 max-h-[360px] overflow-y-auto text-sm leading-relaxed">{transcript}</div>
  {:else}
    <EmptyState title={emptyTitles[phase]} message={emptyMessages[phase]} />
  {/if}
</section>
