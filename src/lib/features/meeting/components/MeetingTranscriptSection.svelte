<script lang="ts">
  import { t } from "svelte-i18n";
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

  type TranscriptPhase = "has_content" | "recording_empty" | "empty";

  const pauseThreshold = 1.5;
  let transcriptBlocks = $derived(
    buildMeetingTranscriptBlocks(segments, recordingSessions, pauseThreshold, liveSessionStartIndex),
  );
  let phase = $derived.by((): TranscriptPhase => {
    if (transcriptBlocks.length > 0) return "has_content";
    if (isRecordingMeeting) return "recording_empty";
    return "empty";
  });
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
    <h3>{$t("meeting_transcript.title")}</h3>
    {#if phase === "has_content" && !isRecordingMeeting}
      <CopyButton text={copyText} />
    {/if}
  </div>

  <div class="p-3 bg-surface-1 rounded-default outline-1 outline-ghost-border text-text-secondary min-h-60 max-h-[480px] overflow-y-auto text-sm leading-relaxed">
    {#if phase === "has_content"}
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
        <span class="text-text-muted">
          {phase === "recording_empty" ? $t("meeting_transcript.listening") : $t("meeting_transcript.no_transcript")}
        </span>
      </div>
    {/if}
  </div>
</section>
