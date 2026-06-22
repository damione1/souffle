<script lang="ts">
  import { Pencil, RotateCcw } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import CopyButton from "../../../components/ui/CopyButton.svelte";
  import type { MeetingRecordingSession, TranscriptionSegment } from "../../../types";
  import { buildMeetingTranscriptBlocks } from "../../../utils";

  let {
    segments,
    recordingSessions,
    liveSessionStartIndex,
    isRecordingMeeting,
    hasEditedTranscript = false,
    isEditing = false,
    editedTranscriptDraft = "",
    onStartEdit,
    onCancelEdit,
    onSaveEdit,
    onSaveAndSummarize,
    onResetEdited,
    onEditDraftChange,
  }: {
    segments: TranscriptionSegment[];
    recordingSessions: MeetingRecordingSession[];
    liveSessionStartIndex: number | null;
    isRecordingMeeting: boolean;
    hasEditedTranscript?: boolean;
    isEditing?: boolean;
    editedTranscriptDraft?: string;
    onStartEdit?: () => void;
    onCancelEdit?: () => void;
    onSaveEdit?: () => void | Promise<void>;
    onSaveAndSummarize?: () => void | Promise<void>;
    onResetEdited?: () => void | Promise<void>;
    onEditDraftChange?: (value: string) => void;
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
          ? `${block.speaker ? (block.speaker === "me" ? "Me" : "Them") + " " : ""}[${block.timestamp}] ${block.text}`
          : `--- ${block.endLabel} ---\n--- ${block.startLabel} ---`,
      )
      .join("\n\n"),
  );
</script>

<section class="flex flex-col gap-3">
  <div class="flex items-center justify-between gap-4 flex-wrap">
    <div class="flex items-center gap-2">
      <h3>{isEditing ? $t("meeting_transcript.editing") : $t("meeting_transcript.title")}</h3>
      {#if hasEditedTranscript && !isEditing}
        <span class="pill pill-success">{$t("meeting_transcript.edited_badge")}</span>
      {/if}
    </div>
    <div class="flex items-center gap-2">
      {#if !isRecordingMeeting && !isEditing && phase === "has_content"}
        {#if hasEditedTranscript && onResetEdited}
          <button
            onclick={onResetEdited}
            class="btn btn-ghost text-xs"
            aria-label={$t("meeting_transcript.reset_to_original")}
          >
            <RotateCcw size={14} />
            {$t("meeting_transcript.reset_to_original")}
          </button>
        {/if}
        {#if onStartEdit}
          <button onclick={onStartEdit} class="btn btn-ghost text-xs" aria-label={$t("meeting_transcript.edit")}>
            <Pencil size={14} />
            {$t("meeting_transcript.edit")}
          </button>
        {/if}
        <CopyButton text={copyText} />
      {/if}
    </div>
  </div>

  {#if isEditing}
    <p class="text-xs text-text-muted">{$t("meeting_transcript.edit_hint")}</p>
    <textarea
      value={editedTranscriptDraft}
      oninput={(e) => onEditDraftChange?.((e.currentTarget as HTMLTextAreaElement).value)}
      class="field-input min-h-60 max-h-[480px] text-sm leading-relaxed resize-y"
    ></textarea>
    <div class="flex gap-2 justify-end">
      <button onclick={onCancelEdit} class="btn btn-ghost">
        {$t("meeting_transcript.cancel_edit")}
      </button>
      <button onclick={onSaveEdit} class="btn btn-secondary">
        {$t("meeting_transcript.save")}
      </button>
      <button onclick={onSaveAndSummarize} class="btn btn-primary">
        {$t("meeting_transcript.save_and_summarize")}
      </button>
    </div>
  {:else}
    <div class="p-3 bg-surface-1 rounded-default outline-1 outline-ghost-border text-text-secondary min-h-40 max-h-[460px] overflow-y-auto text-sm leading-relaxed">
      {#if phase === "has_content"}
        {#each transcriptBlocks as block}
          {#if block.type === "paragraph"}
            <p class="mb-3 last:mb-0 leading-[1.65]">
              {#if block.speaker}
                <span
                  class="mr-1 text-xs font-semibold"
                  class:text-accent={block.speaker === "me"}
                  class:text-text-primary={block.speaker === "them"}
                >{block.speaker === "me" ? $t("transcript.me") : $t("transcript.them")}</span>
              {/if}
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
  {/if}
</section>
