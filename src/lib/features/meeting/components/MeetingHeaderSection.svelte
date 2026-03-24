<script lang="ts">
  import type { MeetingTranscript } from "../../../types";
  import { formatDate, formatDuration } from "../../../utils";

  let {
    meeting,
    isRecordingMeeting,
    segmentCount,
    onBack,
    onNewMeeting,
    onStopRecording,
  }: {
    meeting: MeetingTranscript;
    isRecordingMeeting: boolean;
    segmentCount: number;
    onBack: () => void;
    onNewMeeting: () => void;
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
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" width="16" height="16">
          <path fill-rule="evenodd" d="M17 10a.75.75 0 0 1-.75.75H5.612l4.158 3.96a.75.75 0 1 1-1.04 1.08l-5.5-5.25a.75.75 0 0 1 0-1.08l5.5-5.25a.75.75 0 1 1 1.04 1.08L5.612 9.25H16.25A.75.75 0 0 1 17 10Z" clip-rule="evenodd" />
        </svg>
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
      <span class="pill pill-blue">{meeting.transcription_profile.engine_label}</span>
      <span class="pill pill-muted">{meeting.transcription_profile.model_label}</span>
    </div>
  </div>

  <div class="flex gap-2 shrink-0">
    {#if isRecordingMeeting}
      <button onclick={onStopRecording} class="btn btn-danger">
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" width="16" height="16">
          <path d="M5.25 3A2.25 2.25 0 0 0 3 5.25v9.5A2.25 2.25 0 0 0 5.25 17h9.5A2.25 2.25 0 0 0 17 14.75v-9.5A2.25 2.25 0 0 0 14.75 3h-9.5Z" />
        </svg>
        Stop Recording
      </button>
    {:else}
      <button onclick={onNewMeeting} class="btn">New Meeting</button>
    {/if}
  </div>
</div>
