<script lang="ts">
  import { invoke, Channel } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import type {
    MeetingListItem,
    MeetingTranscript,
    SummarizeProgress,
    OllamaStatus,
    TranscriptionSegment,
  } from "../types";
  import { getAppState } from "../stores/app.svelte";

  const app = getAppState();

  let meetings = $state<MeetingListItem[]>([]);
  let expandedId = $state<string | null>(null);
  let expandedMeeting = $state<MeetingTranscript | null>(null);
  let statusMessage = $state("");
  let ollamaAvailable = $state(false);
  let summaryModels = $state<string[]>([]);
  let selectedModel = $state("");
  let isSummarizing = $state(false);
  let summaryStream = $state("");

  let isRecordingMeeting = $state(false);
  let meetingTitle = $state("");
  let liveMeetingTitle = $state("");
  let liveMeetingTranscript = $state("");
  let liveMeetingSegments = $state<TranscriptionSegment[]>([]);
  let liveMeetingLastTime = 0;

  const pauseThreshold = 1.5;

  function syncSelectedModel(preferredModel?: string | null) {
    if (summaryModels.length === 0) {
      selectedModel = "";
      return;
    }

    if (preferredModel && summaryModels.includes(preferredModel)) {
      selectedModel = preferredModel;
      return;
    }

    if (selectedModel && summaryModels.includes(selectedModel)) {
      return;
    }

    if (app.settings.ollama_model && summaryModels.includes(app.settings.ollama_model)) {
      selectedModel = app.settings.ollama_model;
      return;
    }

    selectedModel = summaryModels[0];
  }

  onMount(async () => {
    await refreshMeetings();
    await checkOllama();
  });

  async function refreshMeetings() {
    try {
      meetings = await invoke("list_meetings");
    } catch (e) {
      statusMessage = String(e);
    }
  }

  async function checkOllama() {
    try {
      const status: OllamaStatus = await invoke("check_ollama");
      ollamaAvailable = status.available;
      summaryModels = status.summary_models;

      if (status.available && status.summary_models.length === 0 && status.models.length > 0) {
        statusMessage =
          "No text-generation Ollama model available for summaries. Install a chat model such as qwen, llama, mistral, gemma, phi, or deepseek.";
        return;
      }

      syncSelectedModel(expandedMeeting?.summary_model);
    } catch {
      ollamaAvailable = false;
    }
  }

  async function toggleExpand(id: string) {
    if (expandedId === id) {
      expandedId = null;
      expandedMeeting = null;
      return;
    }
    try {
      const meeting = await invoke<MeetingTranscript>("get_meeting", { id });
      expandedMeeting = meeting;
      expandedId = id;
      syncSelectedModel(meeting.summary_model);
    } catch (e) {
      statusMessage = String(e);
    }
  }

  async function deleteMeeting(id: string) {
    try {
      await invoke("delete_meeting", { id });
      if (expandedId === id) {
        expandedId = null;
        expandedMeeting = null;
      }
      await refreshMeetings();
    } catch (e) {
      statusMessage = String(e);
    }
  }

  async function summarizeMeeting(id: string) {
    if (!selectedModel) return;
    isSummarizing = true;
    summaryStream = "";
    statusMessage = "";

    try {
      const channel = new Channel<SummarizeProgress>();
      channel.onmessage = (progress) => {
        summaryStream += progress.text;
        if (progress.done) {
          isSummarizing = false;
          invoke("get_meeting", { id }).then((meeting) => {
            expandedMeeting = meeting as MeetingTranscript;
            syncSelectedModel(expandedMeeting.summary_model);
          });
        }
      };
      await invoke("summarize_meeting", { id, model: selectedModel, channel });
    } catch (e) {
      statusMessage = String(e);
      isSummarizing = false;
    }
  }

  async function startMeetingRecording() {
    try {
      const title = meetingTitle.trim() || `Meeting ${new Date().toLocaleDateString()}`;
      liveMeetingTitle = title;
      liveMeetingTranscript = "";
      liveMeetingSegments = [];
      liveMeetingLastTime = 0;
      statusMessage = "";

      const channel = new Channel<TranscriptionSegment>();
      channel.onmessage = (segment) => {
        if (!segment.is_final || !segment.text) return;

        liveMeetingSegments = [...liveMeetingSegments, segment];

        if (liveMeetingTranscript) {
          const gap = segment.start_time - liveMeetingLastTime;
          const endsWithSentence = /[.!?…]\s*$/.test(liveMeetingTranscript);
          if (gap >= pauseThreshold && endsWithSentence && !liveMeetingTranscript.endsWith("\n")) {
            liveMeetingTranscript += "\n\n";
          } else if (
            !liveMeetingTranscript.endsWith(" ") &&
            !liveMeetingTranscript.endsWith("\n") &&
            !segment.text.startsWith(" ")
          ) {
            liveMeetingTranscript += " ";
          }
        }

        liveMeetingTranscript += segment.text;
        liveMeetingLastTime = segment.start_time;
      };
      await invoke("start_meeting_recording", { title, channel });
      isRecordingMeeting = true;
      meetingTitle = "";
    } catch (e) {
      statusMessage = String(e);
      liveMeetingTitle = "";
      liveMeetingTranscript = "";
      liveMeetingSegments = [];
      liveMeetingLastTime = 0;
    }
  }

  async function stopMeetingRecording() {
    try {
      const id: string = await invoke("stop_meeting_recording");
      isRecordingMeeting = false;
      await refreshMeetings();
      expandedMeeting = await invoke("get_meeting", { id });
      expandedId = id;
      liveMeetingTitle = "";
      liveMeetingTranscript = "";
      liveMeetingSegments = [];
      liveMeetingLastTime = 0;
    } catch (e) {
      statusMessage = String(e);
    }
  }

  function formatDuration(seconds: number): string {
    const mins = Math.floor(seconds / 60);
    const secs = Math.floor(seconds % 60);
    return `${mins}:${secs.toString().padStart(2, "0")}`;
  }

  function formatDate(iso: string): string {
    return new Date(iso).toLocaleString();
  }
</script>

<div class="view-stack">
  <section class="surface-card surface-card--compact">
    <div class="panel-header">
      <div>
        <p class="eyebrow">Recordings</p>
        <h3 class="section-title">Meetings</h3>
        <p class="helper-text">Local recordings, transcripts, and optional summaries.</p>
      </div>
      <div class="action-row" style="flex-wrap: wrap;">
        <span class="pill">{meetings.length} meetings</span>
        <span class={`pill ${isRecordingMeeting ? "pill--danger" : "pill--primary"}`}>
          {isRecordingMeeting ? "Recording..." : "Idle"}
        </span>
        <span class={`pill ${ollamaAvailable ? "pill--success" : "pill--warning"}`}>
          {ollamaAvailable ? "Ollama connected" : "Ollama unavailable"}
        </span>
      </div>
    </div>
  </section>

  {#if statusMessage}
    <div class="status-banner status-banner--warning">
      <strong>Status</strong>
      <p class="helper-text">{statusMessage}</p>
    </div>
  {/if}

  <section class="surface-card stack-md">
    <div class="section-header">
      <div>
        <p class="eyebrow">Recorder</p>
        <h3 class="section-title">{isRecordingMeeting ? "Meeting recording in progress." : "Create a new meeting capture."}</h3>
        <p class="section-description">
          Use a descriptive title when needed. For system audio routing, select a virtual device such as BlackHole in Settings first.
        </p>
      </div>
      <span class={`pill ${isRecordingMeeting ? "pill--danger" : "pill--primary"}`}>
        {isRecordingMeeting ? "Recording..." : "Ready to start"}
      </span>
    </div>

    {#if isRecordingMeeting}
      <div class="stack-md">
        <div class="action-row" style="flex-wrap: wrap;">
          <span class="pill pill--danger">{liveMeetingTitle}</span>
          <span class="pill">{liveMeetingSegments.length} segments</span>
        </div>

        <div class="live-output">
          {#if liveMeetingTranscript}
            {liveMeetingTranscript}
          {:else}
            <span class="muted-text">Listening for speech...</span>
          {/if}
        </div>

        <div class="action-row">
          <button onclick={stopMeetingRecording} class="button button-danger">Stop Recording</button>
        </div>
      </div>
    {:else}
      <div class="content-grid">
        <label class="field-group">
          <span class="field-label">Meeting title</span>
          <input
            type="text"
            bind:value={meetingTitle}
            placeholder="Quarterly planning sync"
            class="field-input"
          />
          <p class="helper-text">Leave empty to use the current date as a fallback title.</p>
        </label>

        <div class="surface-card surface-card--muted stack-md" style="padding: 20px;">
          <span class="field-label">Session behavior</span>
          <p class="helper-text">Audio starts immediately, transcript segments stream live, and the finished meeting is saved automatically.</p>
          <button onclick={startMeetingRecording} class="button button-primary">Start Meeting Recording</button>
        </div>
      </div>
    {/if}
  </section>

  <section class="surface-card stack-md">
    <div class="section-header">
      <div>
        <p class="eyebrow">Library</p>
        <h3 class="section-title">Browse saved meetings</h3>
        <p class="section-description">Expand an entry to review the transcript, generate a summary, or remove it from local storage.</p>
      </div>
      <button onclick={refreshMeetings} class="button button-secondary">Refresh list</button>
    </div>

    {#if meetings.length === 0}
      <div class="empty-state">
        <strong>No recordings yet</strong>
        Start a meeting capture to build the transcript library.
      </div>
    {:else}
      <div class="meeting-list">
        {#each meetings as meeting}
          <article class="meeting-item">
            <button onclick={() => toggleExpand(meeting.id)} class="meeting-toggle">
              <div class="list-item-header">
                <div class="stack-sm" style="min-width: 0;">
                  <span class="section-title" style="font-size: 1rem;">{meeting.title}</span>
                  <span class="meeting-meta">{formatDate(meeting.started_at)}</span>
                </div>
                <div class="action-row" style="flex-wrap: wrap; justify-content: flex-end;">
                  <span class="pill">{formatDuration(meeting.duration_seconds)}</span>
                  {#if meeting.has_summary}
                    <span class="pill pill--success">Summarized</span>
                  {/if}
                  <span class="pill">{expandedId === meeting.id ? "Open" : "Closed"}</span>
                </div>
              </div>
            </button>

            {#if expandedId === meeting.id && expandedMeeting}
              <div class="meeting-body stack-lg">
                <div class="content-grid">
                  <div class="stack-sm">
                    <span class="field-label">Transcript</span>
                    <div class="transcript-output scroll-block">
                      {#if expandedMeeting.segments.length > 0}
                        {expandedMeeting.segments.map((segment) => segment.text).join(" ")}
                      {:else}
                        <span class="muted-text">No transcript available.</span>
                      {/if}
                    </div>
                  </div>

                  <div class="stack-sm">
                    <div class="panel-header">
                      <span class="field-label">Summary</span>
                      {#if expandedMeeting.summary_model}
                        <span class="pill">{expandedMeeting.summary_model}</span>
                      {/if}
                    </div>

                    {#if expandedMeeting.summary}
                      <div class="summary-output scroll-block">{expandedMeeting.summary}</div>
                    {:else}
                      <div class="empty-state">
                        <strong>No summary yet</strong>
                        Generate one when a compatible Ollama model is available.
                      </div>
                    {/if}

                    {#if isSummarizing && expandedId === meeting.id}
                      <div class="summary-output scroll-block">{summaryStream}<span class="muted-text">|</span></div>
                    {/if}
                  </div>
                </div>

                {#if ollamaAvailable && summaryModels.length > 0 && expandedMeeting.segments.length > 0}
                  <div class="field-row">
                    <label class="field-group" style="flex: 1;">
                      <span class="field-label">Summary model</span>
                      <select bind:value={selectedModel} disabled={isSummarizing} class="field-select">
                        {#each summaryModels as model}
                          <option value={model}>{model}</option>
                        {/each}
                      </select>
                    </label>

                    <button
                      onclick={() => summarizeMeeting(meeting.id)}
                      disabled={isSummarizing}
                      class="button button-primary"
                    >
                      {expandedMeeting.summary ? "Re-summarize" : "Summarize"}
                    </button>
                  </div>
                {:else if ollamaAvailable && expandedMeeting.segments.length > 0}
                  <div class="status-banner status-banner--warning">
                    <strong>No summary-capable Ollama model detected.</strong>
                    <p class="helper-text">Install a text-generation model to enable reliable meeting summaries.</p>
                  </div>
                {/if}

                <div class="action-row" style="flex-wrap: wrap;">
                  <button
                    onclick={() => {
                      const text = expandedMeeting?.segments.map((segment) => segment.text).join(" ") || "";
                      navigator.clipboard.writeText(text);
                    }}
                    class="button button-secondary"
                  >
                    Copy transcript
                  </button>
                  {#if expandedMeeting.summary}
                    <button
                      onclick={() => navigator.clipboard.writeText(expandedMeeting?.summary || "")}
                      class="button button-secondary"
                    >
                      Copy summary
                    </button>
                  {/if}
                  <button onclick={() => deleteMeeting(meeting.id)} class="button button-ghost danger-text">Delete meeting</button>
                </div>
              </div>
            {/if}
          </article>
        {/each}
      </div>
    {/if}
  </section>
</div>
