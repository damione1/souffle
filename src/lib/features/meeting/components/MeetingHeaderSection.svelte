<script lang="ts">
  import { ArrowLeft, Square } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import type { MeetingTranscript } from "../../../types";
  import { formatDate, formatDuration } from "../../../utils";

  let {
    meeting,
    isRecordingMeeting,
    lockedByDictation,
    segmentCount,
    sessionCount,
    canResumeRecording,
    onBack,
    onNewMeeting,
    onResumeRecording,
    onStopRecording,
  }: {
    meeting: MeetingTranscript;
    isRecordingMeeting: boolean;
    lockedByDictation: boolean;
    segmentCount: number;
    sessionCount: number;
    canResumeRecording: boolean;
    onBack: () => void;
    onNewMeeting: () => void;
    onResumeRecording: () => void | Promise<void>;
    onStopRecording: () => void | Promise<void>;
  } = $props();
</script>

<div class="flex items-start justify-between gap-4 flex-wrap">
  <div class="flex flex-col gap-1">
    {#if isRecordingMeeting}
      <span class="pill pill-danger inline-flex items-center gap-1.5">
        <span class="recording-dot"></span> {$t("meeting_header.recording_badge")}
      </span>
    {:else}
      <button onclick={onBack} class="btn btn-ghost py-1 px-0 text-sm gap-1 mb-1">
        <ArrowLeft size={16} />
        {$t("meeting_header.back_to_history")}
      </button>
    {/if}

    <h2>{meeting.title}</h2>
    <div class="flex items-center gap-2 flex-wrap">
      {#if !isRecordingMeeting}
        <span class="text-sm text-text-muted">{formatDate(meeting.started_at)}</span>
        <span class="pill">{formatDuration(meeting.duration_seconds)}</span>
      {/if}
      <span class="text-sm text-text-muted">{$t("meeting_header.segments_count", { values: { count: segmentCount } })}</span>
      <span class="pill pill-muted">{sessionCount} {sessionCount === 1 ? $t("meeting_header.session_singular") : $t("meeting_header.session_plural")}</span>
      <span class="pill pill-blue">{meeting.transcription_profile.engine_label}</span>
      <span class="pill pill-muted">{meeting.transcription_profile.model_label}</span>
    </div>
  </div>

  <div class="flex gap-2 shrink-0">
    {#if isRecordingMeeting}
      <button onclick={onStopRecording} class="btn btn-danger">
        <Square size={16} />
        {$t("meeting_header.stop_recording")}
      </button>
    {:else}
      {#if canResumeRecording}
        <button onclick={onResumeRecording} disabled={lockedByDictation} class="btn btn-primary">
          {$t("meeting_header.resume_recording")}
        </button>
      {/if}
      <button onclick={onNewMeeting} class="btn">{$t("meeting_header.new_meeting")}</button>
    {/if}
  </div>
</div>

{#if lockedByDictation && !isRecordingMeeting}
  <p class="text-sm text-text-muted">{$t("meeting_header.locked_by_dictation")}</p>
{/if}
