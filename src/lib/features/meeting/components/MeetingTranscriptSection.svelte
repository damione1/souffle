<script lang="ts">
  import CopyButton from "../../../components/ui/CopyButton.svelte";
  import type { TranscriptionSegment } from "../../../types";
  import { groupIntoParagraphs } from "../../../utils";

  let {
    segments,
    isRecordingMeeting,
  }: {
    segments: TranscriptionSegment[];
    isRecordingMeeting: boolean;
  } = $props();

  const pauseThreshold = 1.5;
  let paragraphs = $derived(groupIntoParagraphs(segments, pauseThreshold));
</script>

<section class="surface-card flex flex-col gap-3">
  <div class="flex items-center justify-between gap-4 flex-wrap">
    <h3>Transcript</h3>
    {#if !isRecordingMeeting && paragraphs.length > 0}
      <CopyButton text={paragraphs.map((paragraph) => `[${paragraph.timestamp}] ${paragraph.text}`).join("\n\n")} />
    {/if}
  </div>

  <div class="p-3 bg-surface-1 rounded-default outline-1 outline-ghost-border text-text-secondary min-h-60 max-h-[480px] overflow-y-auto text-sm leading-relaxed">
    {#if paragraphs.length > 0}
      {#each paragraphs as paragraph}
        <p class="mb-3 last:mb-0 leading-[1.65]">
          <span class="text-text-muted text-xs mr-1 tabular-nums">[{paragraph.timestamp}]</span>
          {paragraph.text}
        </p>
      {/each}
    {:else}
      <div class="flex items-center justify-center min-h-[200px]">
        {#if isRecordingMeeting}
          <span class="text-text-muted">Listening for speech...</span>
        {:else}
          <span class="text-text-muted">No transcript available.</span>
        {/if}
      </div>
    {/if}
  </div>
</section>
