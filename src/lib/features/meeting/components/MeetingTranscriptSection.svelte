<script lang="ts">
  import { Pencil, RotateCcw } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import CopyButton from "../../../components/ui/CopyButton.svelte";
  import SpeakerManagePopover from "./SpeakerManagePopover.svelte";
  import type { MeetingRecordingSession, MeetingSpeaker, Speaker, TranscriptionSegment } from "../../../types";
  import {
    buildMeetingTranscriptBlocks,
    persistentSpeakerId,
    resolveSpeakerLabel,
    speakerPillClass,
    speakerPlainLabel,
    type AnchorRect,
  } from "../../../utils";
  import TranscriptWordLine from "./TranscriptWordLine.svelte";

  let {
    segments,
    recordingSessions,
    liveSessionStartIndex,
    isRecordingMeeting,
    speakers = [],
    allSpeakers = [],
    canManageSpeakers = false,
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
    onAddDictionaryAlias,
    onRenameSpeaker,
    onRetagSpeaker,
  }: {
    segments: TranscriptionSegment[];
    recordingSessions: MeetingRecordingSession[];
    liveSessionStartIndex: number | null;
    isRecordingMeeting: boolean;
    /** Persistent speakers referenced by this meeting, for resolving
     * `spk:<id>` labels to a display name; empty for Me/Them-only meetings. */
    speakers?: MeetingSpeaker[];
    /** All persistent speakers in the database, for the retag picker. */
    allSpeakers?: MeetingSpeaker[];
    canManageSpeakers?: boolean;
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
    onAddDictionaryAlias?: (term: string, pronunciation: string | null) => void | Promise<void>;
    onRenameSpeaker?: (id: number, name: string) => void | Promise<void>;
    onRetagSpeaker?: (options: {
      fromSpeakerId: number;
      segmentSortOrders: number[];
      scope: "turn" | "meeting";
      toSpeakerId: number | null;
      newSpeakerName: string | null;
    }) => void | Promise<void>;
  } = $props();

  type TranscriptPhase = "has_content" | "recording_empty" | "empty";
  type OpenSpeakerPopover = {
    speakerId: number;
    speakerName: string;
    segmentSortOrders: number[];
    anchorRect: AnchorRect;
  };

  const pauseThreshold = 1.5;
  let transcriptBlocks = $derived(
    buildMeetingTranscriptBlocks(segments, recordingSessions, pauseThreshold, liveSessionStartIndex),
  );
  let phase = $derived.by((): TranscriptPhase => {
    if (transcriptBlocks.length > 0) return "has_content";
    if (isRecordingMeeting) return "recording_empty";
    return "empty";
  });
  let openSpeakerPopover = $state<OpenSpeakerPopover | null>(null);

  function copyLinePrefix(speaker: Speaker | null | undefined): string {
    const label = speakerPlainLabel(speaker, speakers);
    return label ? `${label} ` : "";
  }

  function segmentSortOrdersForRange(start: number, end: number): number[] {
    return Array.from({ length: end - start }, (_, index) => start + index);
  }

  function speakerDisplayText(
    label: NonNullable<ReturnType<typeof resolveSpeakerLabel>>,
  ): string {
    switch (label.kind) {
      case "me":
        return $t("transcript.me");
      case "them":
        return $t("transcript.them");
      case "named":
        return label.name;
      case "unknown":
        return $t("transcript.speaker_fallback", { values: { id: label.id } });
    }
  }

  function openSpeakerManage(
    speakerId: number,
    speakerName: string,
    segmentRange: { start: number; end: number },
    event: MouseEvent,
  ) {
    if (!canManageSpeakers) return;
    const target = event.currentTarget as HTMLElement;
    openSpeakerPopover = {
      speakerId,
      speakerName,
      segmentSortOrders: segmentSortOrdersForRange(segmentRange.start, segmentRange.end),
      anchorRect: target.getBoundingClientRect(),
    };
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
                  {#if label.kind === "me" || label.kind === "them"}
                    <span
                      class="text-[11.5px] font-semibold"
                      class:text-accent={label.kind === "me"}
                      class:text-secondary={label.kind === "them"}
                    >{speakerDisplayText(label)}</span>
                  {:else}
                    {@const speakerId = persistentSpeakerId(block.speaker) ?? (label.kind === "unknown" ? label.id : 0)}
                    <div class="relative">
                      {#if canManageSpeakers && speakerId > 0}
                        <button
                          type="button"
                          class="speaker-pill cursor-pointer transition-opacity hover:opacity-85 {speakerPillClass(speakerId)}"
                          aria-label={$t("speaker_manage.edit_aria", { values: { name: speakerDisplayText(label) } })}
                          onclick={(event) => openSpeakerManage(speakerId, speakerDisplayText(label), block.segmentRange, event)}
                        >{speakerDisplayText(label)}</button>
                      {:else}
                        <span class="speaker-pill {speakerPillClass(speakerId)}">
                          {speakerDisplayText(label)}
                        </span>
                      {/if}
                      {#if openSpeakerPopover && openSpeakerPopover.speakerId === speakerId}
                        <SpeakerManagePopover
                          speakerId={openSpeakerPopover.speakerId}
                          speakerName={openSpeakerPopover.speakerName}
                          meetingSpeakers={speakers}
                          allSpeakers={allSpeakers}
                          anchorRect={openSpeakerPopover.anchorRect}
                          onClose={() => { openSpeakerPopover = null; }}
                          onRename={(name) => onRenameSpeaker?.(openSpeakerPopover!.speakerId, name)}
                          onRetag={(options) => onRetagSpeaker?.({
                            fromSpeakerId: openSpeakerPopover!.speakerId,
                            segmentSortOrders: openSpeakerPopover!.segmentSortOrders,
                            ...options,
                          })}
                        />
                      {/if}
                    </div>
                  {/if}
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
              {#if onAddDictionaryAlias}
                <TranscriptWordLine
                  text={block.text}
                  onAddAlias={onAddDictionaryAlias}
                  class="m-0 block text-sm leading-[1.7] text-text-secondary"
                />
              {:else}
                <p class="m-0 text-sm leading-[1.7] text-text-secondary">{block.text}</p>
              {/if}
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
