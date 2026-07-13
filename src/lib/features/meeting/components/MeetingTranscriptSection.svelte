<script lang="ts">
  import { Pencil, RotateCcw } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import CopyButton from "../../../components/ui/CopyButton.svelte";
  import type { MeetingRecordingSession, MeetingSpeaker, Speaker, TranscriptionSegment } from "../../../types";
  import { buildMeetingTranscriptBlocks, resolveSpeakerLabel, speakerPlainLabel } from "../../../utils";

  let {
    segments,
    recordingSessions,
    liveSessionStartIndex,
    isRecordingMeeting,
    speakers = [],
    hasEditedTranscript = false,
    isEditing = false,
    editedTranscriptDraft = "",
    onStartEdit,
    onCancelEdit,
    onSaveEdit,
    onSaveAndSummarize,
    onResetEdited,
    onEditDraftChange,
    onParagraphClick,
  }: {
    segments: TranscriptionSegment[];
    recordingSessions: MeetingRecordingSession[];
    liveSessionStartIndex: number | null;
    isRecordingMeeting: boolean;
    /** Persistent speakers referenced by this meeting, for resolving
     * `spk:<id>` labels to a display name; empty for Me/Them-only meetings. */
    speakers?: MeetingSpeaker[];
    hasEditedTranscript?: boolean;
    isEditing?: boolean;
    editedTranscriptDraft?: string;
    onStartEdit?: () => void;
    onCancelEdit?: () => void;
    onSaveEdit?: () => void | Promise<void>;
    onSaveAndSummarize?: () => void | Promise<void>;
    onResetEdited?: () => void | Promise<void>;
    onEditDraftChange?: (value: string) => void;
    /** A paragraph's timestamp was clicked; seeks the audio player if that
     * paragraph maps to a recorded session (no-op otherwise). */
    onParagraphClick?: (recordingSessionIndex: number | null, startTime: number) => void;
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
  function copyLinePrefix(speaker: Speaker | null | undefined): string {
    const label = speakerPlainLabel(speaker, speakers);
    return label ? `${label} ` : "";
  }

  let copyText = $derived(
    transcriptBlocks
      .map((block) =>
        block.type === "paragraph"
          ? `${copyLinePrefix(block.speaker)}[${block.timestamp}] ${block.text}`
          : `--- ${block.endLabel} ---\n--- ${block.startLabel} ---`,
      )
      .join("\n\n"),
  );
</script>

<section class="surface-card flex flex-col gap-3">
  <div class="flex items-center justify-between gap-4 flex-wrap">
    <div class="flex items-center gap-2">
      <h3 class="text-sm font-semibold text-text-primary">
        {isEditing ? $t("meeting_transcript.editing") : $t("meeting_transcript.title")}
        <span class="text-[12.5px] font-normal text-text-muted">
          · {$t("meeting_transcript.segments_count", { values: { count: segments.length } })}
        </span>
      </h3>
      {#if hasEditedTranscript && !isEditing}
        <span class="pill pill-success">{$t("meeting_transcript.edited_badge")}</span>
      {/if}
    </div>
    <div class="flex items-center gap-2">
      {#if !isRecordingMeeting && !isEditing && phase === "has_content"}
        {#if hasEditedTranscript && onResetEdited}
          <button
            onclick={onResetEdited}
            class="btn btn-ghost gap-1.5 px-2.5 py-[5px] text-[12.5px]"
            aria-label={$t("meeting_transcript.reset_to_original")}
          >
            <RotateCcw size={14} />
            {$t("meeting_transcript.reset_to_original")}
          </button>
        {/if}
        {#if onStartEdit}
          <button
            onclick={onStartEdit}
            class="btn btn-ghost gap-1.5 px-2.5 py-[5px] text-[12.5px]"
            aria-label={$t("meeting_transcript.edit")}
          >
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
      <button onclick={onSaveEdit} class="btn">
        {$t("meeting_transcript.save")}
      </button>
      <button onclick={onSaveAndSummarize} class="btn btn-primary">
        {$t("meeting_transcript.save_and_summarize")}
      </button>
    </div>
  {:else}
    <div class="flex min-h-[236px] max-h-[300px] flex-col gap-4 overflow-y-auto pr-1.5">
      {#if phase === "has_content"}
        {#each transcriptBlocks as block}
          {#if block.type === "paragraph"}
            {@const label = resolveSpeakerLabel(block.speaker, speakers)}
            <div class="flex flex-col gap-[3px]">
              <div class="flex items-center gap-2">
                {#if label}
                  <span
                    class="text-[11.5px] font-semibold"
                    class:text-accent={label.kind === "me"}
                    class:text-secondary={label.kind === "them"}
                  >{label.kind === "me"
                    ? $t("transcript.me")
                    : label.kind === "them"
                      ? $t("transcript.them")
                      : label.kind === "named"
                        ? label.name
                        : $t("transcript.speaker_fallback", { values: { id: label.id } })}</span>
                {/if}
                {#if onParagraphClick}
                  <button
                    type="button"
                    class="font-mono text-[10.5px] text-text-faint hover:text-accent hover:underline"
                    onclick={() => onParagraphClick?.(block.recordingSessionIndex, block.startTime)}
                    aria-label={$t("meeting_audio.seek_to", { values: { timestamp: block.timestamp } })}
                  >{block.timestamp}</button>
                {:else}
                  <span class="font-mono text-[10.5px] text-text-faint">{block.timestamp}</span>
                {/if}
              </div>
              <p class="m-0 text-sm leading-[1.7] text-text-secondary">{block.text}</p>
            </div>
          {:else}
            <div class="my-1 flex items-center gap-3 text-text-muted/80">
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
        <div class="flex flex-1 items-center justify-center">
          <span class="text-text-muted">
            {phase === "recording_empty" ? $t("meeting_transcript.listening") : $t("meeting_transcript.no_transcript")}
          </span>
        </div>
      {/if}
    </div>
  {/if}
</section>
