<script lang="ts">
  import { ArrowLeft, Square } from "@lucide/svelte";
  import type { MeetingTranscript } from "../../../types";
  import { formatDate, formatDuration } from "../../../utils";

  let {
    meeting,
    isRecordingMeeting,
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
        <span class="recording-dot"></span> Recording
      </span>
    {:else}
      <button onclick={onBack} class="btn btn-ghost py-1 px-0 text-sm gap-1 mb-1">
        <ArrowLeft size={16} />
        Back to history
      </button>
    {/if}

    <h2>{meeting.title}</h2>
    <div class="flex items-center gap-2 flex-wrap">
      {#if !isRecordingMeeting}
        <span class="text-sm text-text-muted">{formatDate(meeting.started_at)}</span>
        <span class="pill">{formatDuration(meeting.duration_seconds)}</span>
      {/if}
      <span class="text-sm text-text-muted">{segmentCount} segments</span>
      <span class="pill pill-muted">{sessionCount} {sessionCount === 1 ? "session" : "sessions"}</span>
      <span class="pill pill-blue">{meeting.transcription_profile.engine_label}</span>
      <span class="pill pill-muted">{meeting.transcription_profile.model_label}</span>
    </div>
  </div>

  <div class="flex gap-2 shrink-0">
    {#if isRecordingMeeting}
      <button onclick={onStopRecording} class="btn btn-danger">
        <Square size={16} />
        Stop Recording
      </button>
    {:else}
      {#if canResumeRecording}
        <button onclick={onResumeRecording} class="btn btn-primary">Resume Recording</button>
      {/if}
      <button onclick={onNewMeeting} class="btn">New Meeting</button>
    {/if}
  </div>
</div>
