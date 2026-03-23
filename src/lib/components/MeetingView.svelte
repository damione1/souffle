<script lang="ts">
  import { invoke, Channel } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import type {
    MeetingTranscript,
    SummarizeProgress,
    OllamaStatus,
    TranscriptionSegment,
  } from "../types";
  import { getAppState } from "../stores/app.svelte";
  import { formatTimestamp, formatDate, formatDuration, groupIntoParagraphs, errorMessage } from "../utils";
  import type { Paragraph } from "../utils";
  import StatusBanner from "./ui/StatusBanner.svelte";
  import CopyButton from "./ui/CopyButton.svelte";
  import EmptyState from "./ui/EmptyState.svelte";
  import ConfirmAction from "./ui/ConfirmAction.svelte";
  import Spinner from "./ui/Spinner.svelte";

  const app = getAppState();

  let statusMessage = $state("");
  let ollamaAvailable = $state(false);
  let summaryModels = $state<string[]>([]);
  let selectedModel = $state("");
  let isSummarizing = $state(false);
  let summaryStream = $state("");

  let isRecordingMeeting = $state(false);
  let meetingTitle = $state("");
  let liveMeetingSegments = $state<TranscriptionSegment[]>([]);
  let liveMeetingLastTime = 0;

  // The meeting being viewed (loaded from backend or just finished)
  let meeting = $state<MeetingTranscript | null>(null);
  let isLoadingMeeting = $state(false);

  const pauseThreshold = 1.5;

  // Determine if we're in "new meeting" form mode
  let isNewMode = $derived(!app.currentMeetingId && !isRecordingMeeting);

  function syncSelectedModel(preferredModel?: string | null) {
    if (summaryModels.length === 0) { selectedModel = ""; return; }
    if (preferredModel && summaryModels.includes(preferredModel)) { selectedModel = preferredModel; return; }
    if (selectedModel && summaryModels.includes(selectedModel)) return;
    if (app.settings.ollama_model && summaryModels.includes(app.settings.ollama_model)) {
      selectedModel = app.settings.ollama_model;
      return;
    }
    selectedModel = summaryModels[0];
  }

  onMount(async () => {
    await checkOllama();
    // If navigated here with a meeting ID, load it
    if (app.currentMeetingId) {
      await loadMeeting(app.currentMeetingId);
    }
  });

  // React to external meeting ID changes (e.g. from history view)
  $effect(() => {
    const id = app.currentMeetingId;
    if (id && (!meeting || meeting.id !== id) && !isRecordingMeeting) {
      loadMeeting(id);
    }
  });

  async function loadMeeting(id: string) {
    isLoadingMeeting = true;
    try {
      meeting = await invoke<MeetingTranscript>("get_meeting", { id });
      syncSelectedModel(meeting.summary_model);
    } catch (e) {
      statusMessage = errorMessage(e);
    } finally {
      isLoadingMeeting = false;
    }
  }

  async function checkOllama() {
    try {
      const status: OllamaStatus = await invoke("check_ollama");
      ollamaAvailable = status.available;
      summaryModels = status.summary_models;
      syncSelectedModel(meeting?.summary_model);
    } catch {
      ollamaAvailable = false;
    }
  }

  async function startMeetingRecording() {
    try {
      const title = meetingTitle.trim() || `Meeting ${new Date().toLocaleDateString()}`;
      liveMeetingSegments = [];
      liveMeetingLastTime = 0;
      statusMessage = "";
      meeting = null;

      const channel = new Channel<TranscriptionSegment>();
      channel.onmessage = (segment) => {
        if (!segment.is_final || !segment.text) return;
        liveMeetingSegments = [...liveMeetingSegments, segment];
        liveMeetingLastTime = segment.start_time;
      };
      await invoke("start_meeting_recording", { title, channel });
      isRecordingMeeting = true;
      // Create a temporary meeting object for display during recording
      meeting = {
        id: "",
        title,
        started_at: new Date().toISOString(),
        ended_at: null,
        duration_seconds: 0,
        engine: "kyutai-stt",
        segments: [],
        summary: null,
        summary_model: null,
        summary_generated_at: null,
      };
      meetingTitle = "";
    } catch (e) {
      statusMessage = errorMessage(e);
      liveMeetingSegments = [];
      liveMeetingLastTime = 0;
    }
  }

  async function stopMeetingRecording() {
    try {
      const id: string = await invoke("stop_meeting_recording");
      isRecordingMeeting = false;
      app.currentMeetingId = id;
      meeting = await invoke("get_meeting", { id });
      syncSelectedModel(meeting?.summary_model);
      liveMeetingSegments = [];
      liveMeetingLastTime = 0;
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  async function summarizeMeeting() {
    if (!selectedModel || !meeting || !meeting.id) return;
    isSummarizing = true;
    summaryStream = "";
    statusMessage = "";

    try {
      const channel = new Channel<SummarizeProgress>();
      channel.onmessage = (progress) => {
        summaryStream += progress.text;
        if (progress.done) {
          isSummarizing = false;
          invoke("get_meeting", { id: meeting!.id }).then((m) => {
            meeting = m as MeetingTranscript;
            syncSelectedModel(meeting!.summary_model);
          });
        }
      };
      await invoke("summarize_meeting", { id: meeting.id, model: selectedModel, channel });
    } catch (e) {
      statusMessage = errorMessage(e);
      isSummarizing = false;
    }
  }

  async function deleteMeeting() {
    if (!meeting || !meeting.id) return;
    try {
      await invoke("delete_meeting", { id: meeting.id });
      meeting = null;
      app.currentMeetingId = null;
      // Go back to history
      app.currentView = "meeting-history";
    } catch (e) {
      statusMessage = errorMessage(e);
    }
  }

  // Get the segments to display (live during recording, or from loaded meeting)
  let displaySegments = $derived(isRecordingMeeting ? liveMeetingSegments : (meeting?.segments ?? []));

  let displayParagraphs = $derived(groupIntoParagraphs(displaySegments, pauseThreshold));

  export function getRecordingState() {
    return isRecordingMeeting;
  }
</script>

<div class="flex flex-col gap-4">
  {#if statusMessage}
    <StatusBanner message={statusMessage} variant="warning" />
  {/if}

  {#if isNewMode}
    <!-- New Meeting Form (simplified, centered, no card) -->
    <div class="flex flex-col items-center justify-center h-full gap-6">
      <input
        type="text"
        bind:value={meetingTitle}
        placeholder="Meeting title (optional)"
        class="field-input w-full max-w-sm text-center"
        onkeydown={(e) => { if (e.key === "Enter") startMeetingRecording(); }}
      />
      <button onclick={startMeetingRecording} class="btn btn-primary btn-lg">
        Start Recording
      </button>
      <p class="text-sm text-text-muted">Leave empty to use the current date</p>
    </div>

  {:else if isLoadingMeeting}
    <div class="flex flex-col items-center gap-2 p-8 text-text-muted">
      <Spinner />
      <p class="text-sm">Loading meeting...</p>
    </div>

  {:else if meeting}
    <!-- Meeting Item Page -->
    <div class="flex items-start justify-between gap-4 flex-wrap">
      <div class="flex flex-col gap-1">
        {#if isRecordingMeeting}
          <span class="pill pill-danger inline-flex items-center gap-1.5">
            <span class="recording-dot"></span> Recording
          </span>
        {:else}
          <button onclick={() => { app.currentMeetingId = null; app.currentView = "meeting-history"; }} class="btn btn-ghost py-1 px-0 text-sm gap-1 mb-1">
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
          <span class="text-sm text-text-muted">{displaySegments.length} segments</span>
        </div>
      </div>

      <div class="flex gap-2 shrink-0">
        {#if isRecordingMeeting}
          <button onclick={stopMeetingRecording} class="btn btn-danger">
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" width="16" height="16">
              <path d="M5.25 3A2.25 2.25 0 0 0 3 5.25v9.5A2.25 2.25 0 0 0 5.25 17h9.5A2.25 2.25 0 0 0 17 14.75v-9.5A2.25 2.25 0 0 0 14.75 3h-9.5Z" />
            </svg>
            Stop Recording
          </button>
        {:else}
          <button onclick={() => app.newMeeting()} class="btn">New Meeting</button>
        {/if}
      </div>
    </div>

    <!-- Key Insights (shown after recording, if summary exists) -->
    {#if !isRecordingMeeting && meeting.summary}
      {@const lines = meeting.summary.split("\n").filter((l) => l.trim())}
      {@const keyPoints = lines.filter((l) => /^[-•*]\s/.test(l.trim()) || /^\d+[.)]\s/.test(l.trim())).slice(0, 4)}
      {#if keyPoints.length > 0}
        <div class="grid grid-cols-[repeat(auto-fill,minmax(200px,1fr))] gap-3">
          {#each keyPoints as point, i}
            <div class="flex gap-3 p-3.5 bg-surface-2 rounded-default outline-1 outline-ghost-border items-start">
              <span class="flex items-center justify-center w-6 h-6 rounded-full bg-accent-blue/15 text-accent-blue text-xs font-bold shrink-0">{i + 1}</span>
              <p class="m-0 text-sm text-text-secondary leading-normal">{point.replace(/^[-•*\d.)]+\s*/, "").trim()}</p>
            </div>
          {/each}
        </div>
      {/if}
    {/if}

    <!-- Two-column: Transcript + Summary -->
    <div class="grid grid-cols-2 gap-4 max-[700px]:grid-cols-1">
      <!-- Left: Transcript -->
      <section class="surface-card flex flex-col gap-3">
        <div class="flex items-center justify-between gap-4 flex-wrap">
          <h3>Transcript</h3>
          {#if !isRecordingMeeting && displayParagraphs.length > 0}
            <CopyButton text={displayParagraphs.map((p) => `[${p.timestamp}] ${p.text}`).join("\n\n")} />
          {/if}
        </div>
        <div class="p-3 bg-surface-1 rounded-default outline-1 outline-ghost-border text-text-secondary min-h-60 max-h-[480px] overflow-y-auto text-sm leading-relaxed">
          {#if displayParagraphs.length > 0}
            {#each displayParagraphs as para}
              <p class="mb-3 last:mb-0 leading-[1.65]">
                <span class="text-text-muted text-xs mr-1 tabular-nums">[{para.timestamp}]</span>
                {para.text}
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

      <!-- Right: Summary -->
      <section class="surface-card flex flex-col gap-3">
        <div class="flex items-center justify-between gap-4 flex-wrap">
          <h3>Summary</h3>
          {#if meeting.summary && !isRecordingMeeting}
            <CopyButton text={meeting.summary} />
          {/if}
        </div>

        {#if isRecordingMeeting}
          <EmptyState message="Stop the recording to generate a summary.">
            {#snippet icon()}
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" width="32" height="32">
                <path stroke-linecap="round" stroke-linejoin="round" d="M12 6v6h4.5m4.5 0a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
              </svg>
            {/snippet}
          </EmptyState>
        {:else if meeting.summary}
          <div class="p-3 bg-surface-1 rounded-default outline-1 outline-ghost-border text-text-secondary whitespace-pre-wrap min-h-[100px] max-h-[360px] overflow-y-auto text-sm leading-relaxed">{meeting.summary}</div>
          {#if meeting.summary_model}
            <span class="pill pill-muted self-start">Generated with {meeting.summary_model}</span>
          {/if}
        {:else}
          <EmptyState message="No summary yet. Generate one below.">
            {#snippet icon()}
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" width="32" height="32">
                <path stroke-linecap="round" stroke-linejoin="round" d="M9.813 15.904 9 18.75l-.813-2.846a4.5 4.5 0 0 0-3.09-3.09L2.25 12l2.846-.813a4.5 4.5 0 0 0 3.09-3.09L9 5.25l.813 2.846a4.5 4.5 0 0 0 3.09 3.09L15.75 12l-2.846.813a4.5 4.5 0 0 0-3.09 3.09ZM18.259 8.715 18 9.75l-.259-1.035a3.375 3.375 0 0 0-2.455-2.456L14.25 6l1.036-.259a3.375 3.375 0 0 0 2.455-2.456L18 2.25l.259 1.035a3.375 3.375 0 0 0 2.455 2.456L21.75 6l-1.036.259a3.375 3.375 0 0 0-2.455 2.456ZM16.894 20.567 16.5 21.75l-.394-1.183a2.25 2.25 0 0 0-1.423-1.423L13.5 18.75l1.183-.394a2.25 2.25 0 0 0 1.423-1.423l.394-1.183.394 1.183a2.25 2.25 0 0 0 1.423 1.423l1.183.394-1.183.394a2.25 2.25 0 0 0-1.423 1.423Z" />
              </svg>
            {/snippet}
          </EmptyState>
        {/if}

        {#if isSummarizing}
          <div class="p-3 bg-surface-1 rounded-default outline-1 outline-ghost-border text-text-secondary whitespace-pre-wrap min-h-[80px] overflow-y-auto text-sm leading-relaxed">{summaryStream}<span class="text-text-muted animate-pulse">|</span></div>
        {/if}

        <!-- Generate Summary CTA -->
        {#if !isRecordingMeeting && displaySegments.length > 0}
          {#if ollamaAvailable && summaryModels.length > 0}
            <div class="flex gap-2 items-center">
              <select bind:value={selectedModel} disabled={isSummarizing} class="field-select">
                {#each summaryModels as model}
                  <option value={model}>{model}</option>
                {/each}
              </select>
              <button
                onclick={summarizeMeeting}
                disabled={isSummarizing}
                class="btn btn-primary"
              >
                {#if isSummarizing}
                  <Spinner />
                  Generating...
                {:else}
                  {meeting.summary ? "Re-generate Summary" : "Generate Summary"}
                {/if}
              </button>
            </div>
          {:else if !ollamaAvailable}
            <div class="flex items-center gap-2 py-2">
              <span class="status-dot"></span>
              <span class="text-sm text-text-muted">Connect Ollama in Settings to enable summaries.</span>
            </div>
          {/if}
        {/if}
      </section>
    </div>

    <!-- Footer actions for completed meetings -->
    {#if !isRecordingMeeting && meeting.id}
      <div class="flex items-center gap-2 pt-2 border-t border-ghost-border">
        <ConfirmAction
          label="Delete meeting"
          confirmLabel="Yes, delete"
          confirmMessage="Delete this meeting permanently?"
          variant="danger"
          onConfirm={deleteMeeting}
        />
      </div>
    {/if}
  {/if}
</div>
