<script lang="ts">
  import CopyButton from "../../../components/ui/CopyButton.svelte";
  import type { MeetingRecordingSession, TranscriptionSegment } from "../../../types";
  import { buildMeetingTranscriptBlocks } from "../../../utils";

  let {
    segments,
    recordingSessions,
    liveSessionStartIndex,
    isRecordingMeeting,
  }: {
    segments: TranscriptionSegment[];
    recordingSessions: MeetingRecordingSession[];
    liveSessionStartIndex: number | null;
    isRecordingMeeting: boolean;
  } = $props();

  const pauseThreshold = 1.5;
  let transcriptBlocks = $derived(
    buildMeetingTranscriptBlocks(segments, recordingSessions, pauseThreshold, liveSessionStartIndex),
  );
  let copyText = $derived(
    transcriptBlocks
      .map((block) =>
        block.type === "paragraph"
          ? `[${block.timestamp}] ${block.text}`
          : `--- ${block.endLabel} ---\n--- ${block.startLabel} ---`,
      )
      .join("\n\n"),
  );
</script>

<section class="surface-card flex flex-col gap-3">
  <div class="flex items-center justify-between gap-4 flex-wrap">
    <h3>Transcript</h3>
    {#if !isRecordingMeeting && transcriptBlocks.length > 0}
      <CopyButton text={copyText} />
    {/if}
  </div>

  <div class="p-3 bg-surface-1 rounded-default outline-1 outline-ghost-border text-text-secondary min-h-60 max-h-[480px] overflow-y-auto text-sm leading-relaxed">
    {#if transcriptBlocks.length > 0}
      {#each transcriptBlocks as block}
        {#if block.type === "paragraph"}
          <p class="mb-3 last:mb-0 leading-[1.65]">
            <span class="text-text-muted text-xs mr-1 tabular-nums">[{block.timestamp}]</span>
            {block.text}
          </p>
        {:else}
          <div class="my-3 flex items-center gap-3 text-text-muted/80">
            <div class="h-px flex-1 bg-ghost-border"></div>
            <div class="flex flex-col items-center gap-0.5 text-center">
              <p class="m-0 text-[0.625rem] font-medium uppercase tracking-[0.16em] text-text-muted/75">{block.endLabel}</p>
              <p class="m-0 text-xs font-medium text-text-muted">{block.startLabel}</p>
            </div>
            <div class="h-px flex-1 bg-ghost-border"></div>
          </div>
        {/if}
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
