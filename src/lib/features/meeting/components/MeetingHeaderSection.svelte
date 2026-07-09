<script lang="ts">
  import { ArrowLeft, Pencil, Square, Users } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import type { MeetingTranscript } from "../../../types";
  import { formatDate, formatDuration } from "../../../utils";
  import Spinner from "../../../components/ui/Spinner.svelte";

  let {
    meeting,
    isRecordingMeeting,
    isStopping,
    systemAudioStatus,
    lockedByDictation,
    segmentCount,
    sessionCount,
    canResumeRecording,
    onBack,
    onRename,
    onResumeRecording,
    onStopRecording,
  }: {
    meeting: MeetingTranscript;
    isRecordingMeeting: boolean;
    isStopping: boolean;
    systemAudioStatus: import("../../../types").SystemAudioStatus | null;
    lockedByDictation: boolean;
    segmentCount: number;
    sessionCount: number;
    canResumeRecording: boolean;
    onBack: () => void;
    onRename: (title: string) => void;
    onResumeRecording: () => void | Promise<void>;
    onStopRecording: () => void | Promise<void>;
  } = $props();

  let isEditingTitle = $state(false);
  let titleDraft = $state("");
  let titleInput: HTMLInputElement | undefined = $state();

  function startTitleEdit() {
    titleDraft = meeting.title;
    isEditingTitle = true;
    queueMicrotask(() => titleInput?.focus());
  }

  function commitTitle() {
    isEditingTitle = false;
    if (titleDraft.trim() && titleDraft.trim() !== meeting.title) {
      onRename(titleDraft);
    }
  }

  function onTitleKeydown(event: KeyboardEvent) {
    if (event.key === "Enter") commitTitle();
    if (event.key === "Escape") isEditingTitle = false;
  }
</script>

{#if !isRecordingMeeting}
  <button
    onclick={onBack}
    class="btn btn-ghost -ml-1.5 gap-1.5 self-start px-2.5 py-1 text-[13px]"
  >
    <ArrowLeft size={16} aria-hidden="true" />
    {$t("meeting_header.back_to_history")}
  </button>
{/if}

<div class="flex items-start justify-between gap-4 flex-wrap">
  <div class="flex flex-col gap-[9px]">
    {#if isRecordingMeeting}
      <span class="inline-flex items-center gap-2 self-start rounded-full bg-danger/13 px-[13px] py-1.5 text-[12.5px] font-semibold text-danger-soft outline-1 outline-danger/28">
        <span class="recording-dot"></span> {$t("meeting_header.recording_badge")}
        {#if systemAudioStatus}
          {#if systemAudioStatus.active}
            <span class="font-normal text-text-muted">· {$t("meeting_header.system_audio_active")}</span>
          {:else}
            <span class="font-normal text-text-muted" title={systemAudioStatus.reason ?? ""}>· {$t("meeting_header.system_audio_unavailable")}</span>
          {/if}
        {/if}
      </span>
    {/if}

    {#if isEditingTitle}
      <input
        bind:this={titleInput}
        bind:value={titleDraft}
        onblur={commitTitle}
        onkeydown={onTitleKeydown}
        class="field-input font-heading text-[23px] font-bold"
        aria-label={$t("meeting_header.rename_aria")}
      />
    {:else}
      <button
        onclick={startTitleEdit}
        class="group flex items-center gap-[9px] text-left cursor-text"
        aria-label={$t("meeting_header.rename_aria")}
      >
        <h1 class="text-[23px] font-bold text-text-primary">{meeting.title}</h1>
        <Pencil
          size={15}
          class="text-text-muted opacity-0 transition-opacity group-hover:opacity-100"
          aria-hidden="true"
        />
      </button>
    {/if}
    <div class="flex items-center gap-[9px] flex-wrap text-[12.5px] text-text-muted">
      {#if !isRecordingMeeting}
        <span>{formatDate(meeting.started_at)}</span>
        <span class="text-text-faint">·</span>
        <span>{formatDuration(meeting.duration_seconds)}</span>
        <span class="text-text-faint">·</span>
      {/if}
      <span>{$t("meeting_header.segments_count", { values: { count: segmentCount } })}</span>
      {#if sessionCount > 1}
        <span class="text-text-faint">·</span>
        <span>{sessionCount} {$t("meeting_header.session_plural")}</span>
      {/if}
      <span
        class="ml-0.5 rounded-full bg-secondary/10 px-2 py-0.5 text-[11px] text-secondary outline-1 outline-secondary/20"
        title={meeting.transcription_profile.engine_label}
      >{meeting.transcription_profile.model_label}</span>
    </div>
    {#if meeting.participants.length > 0}
      <div class="mt-px flex items-center gap-[7px] flex-wrap">
        <Users size={13} class="shrink-0 text-text-muted" aria-hidden="true" />
        {#each meeting.participants as participant (participant.name + (participant.email ?? ""))}
          <span
            class="rounded-full bg-surface-2 px-[9px] py-[2.5px] text-[11.5px] text-text-tertiary outline-1 outline-ghost-border"
            title={participant.email ?? ""}
          >
            {participant.name}{participant.is_organizer
              ? ` (${$t("calendar.organizer")})`
              : ""}{participant.is_current_user ? ` (${$t("calendar.you")})` : ""}
          </span>
        {/each}
      </div>
    {/if}
  </div>

  <div class="flex gap-2 shrink-0">
    {#if isRecordingMeeting}
      <button
        onclick={onStopRecording}
        disabled={isStopping}
        class="inline-flex cursor-pointer items-center gap-2 rounded-[11px] bg-danger px-4 py-[9px] text-[13.5px] font-semibold text-on-danger transition-colors hover:bg-danger/90 disabled:cursor-default disabled:opacity-60"
      >
        {#if isStopping}
          <Spinner />
          {$t("home.stopping")}
        {:else}
          <Square size={13} fill="currentColor" aria-hidden="true" />
          {$t("meeting_header.stop_recording")}
        {/if}
      </button>
    {:else}
      {#if canResumeRecording}
        <button onclick={onResumeRecording} disabled={lockedByDictation} class="btn btn-primary">
          {$t("meeting_header.resume_recording")}
        </button>
      {/if}
    {/if}
  </div>
</div>

{#if lockedByDictation && !isRecordingMeeting}
  <p class="text-sm text-text-muted">{$t("meeting_header.locked_by_dictation")}</p>
{/if}
