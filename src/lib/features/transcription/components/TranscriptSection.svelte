<script lang="ts">
  import CopyButton from "../../../components/ui/CopyButton.svelte";
  import EmptyState from "../../../components/ui/EmptyState.svelte";

  let {
    transcript,
    isStartingRecording,
    isRecording,
  }: {
    transcript: string;
    isStartingRecording: boolean;
    isRecording: boolean;
  } = $props();
</script>

<section class="surface-card flex flex-col gap-3">
  <div class="flex items-center justify-between gap-4 flex-wrap">
    <h3>Transcript</h3>
    {#if transcript}
      <CopyButton text={transcript} />
    {/if}
  </div>

  {#if transcript}
    <div class="p-3 bg-surface-1 rounded-default outline-1 outline-ghost-border text-text-secondary whitespace-pre-wrap min-h-40 max-h-[360px] overflow-y-auto text-sm leading-relaxed">{transcript}</div>
  {:else}
    <EmptyState
      title={isStartingRecording ? "Warming up..." : isRecording ? "Listening for speech" : "No transcript yet"}
      message={isRecording || isStartingRecording ? "Text will appear as segments arrive." : "Press the mic button to start."}
    />
  {/if}
</section>
