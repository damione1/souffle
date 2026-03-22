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
      statusMessage = String(e);
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
      statusMessage = String(e);
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
      statusMessage = String(e);
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
      statusMessage = String(e);
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
      statusMessage = String(e);
    }
  }

  function formatTimestamp(seconds: number): string {
    const mins = Math.floor(seconds / 60);
    const secs = Math.floor(seconds % 60);
    return `${mins}:${secs.toString().padStart(2, "0")}`;
  }

  function formatDate(iso: string): string {
    return new Date(iso).toLocaleString();
  }

  function formatDuration(seconds: number): string {
    const mins = Math.floor(seconds / 60);
    const secs = Math.floor(seconds % 60);
    return `${mins}m ${secs}s`;
  }

  // Get the segments to display (live during recording, or from loaded meeting)
  let displaySegments = $derived(isRecordingMeeting ? liveMeetingSegments : (meeting?.segments ?? []));

  type Paragraph = { timestamp: string; text: string };

  /** Group word-level segments into flowing paragraphs with a leading timestamp */
  function groupIntoParagraphs(segments: TranscriptionSegment[]): Paragraph[] {
    if (segments.length === 0) return [];

    const paragraphs: Paragraph[] = [];
    let currentTimestamp = formatTimestamp(segments[0].start_time);
    let currentWords: string[] = [];
    let lastTime = segments[0].start_time;
    let lastText = "";

    for (const seg of segments) {
      const gap = seg.start_time - lastTime;
      const endsWithSentence = /[.!?…]\s*$/.test(lastText);

      if (currentWords.length > 0 && gap >= pauseThreshold && endsWithSentence) {
        paragraphs.push({ timestamp: currentTimestamp, text: currentWords.join(" ") });
        currentTimestamp = formatTimestamp(seg.start_time);
        currentWords = [];
      }

      currentWords.push(seg.text.trim());
      lastTime = seg.start_time;
      lastText = seg.text;
    }

    if (currentWords.length > 0) {
      paragraphs.push({ timestamp: currentTimestamp, text: currentWords.join(" ") });
    }

    return paragraphs;
  }

  let displayParagraphs = $derived(groupIntoParagraphs(displaySegments));

  let confirmDelete = $state(false);

  export function getRecordingState() {
    return isRecordingMeeting;
  }
</script>

<div class="view">
  {#if statusMessage}
    <div class="status-banner warning">
      <p class="text-sm">{statusMessage}</p>
    </div>
  {/if}

  {#if isNewMode}
    <!-- ═══ New Meeting Form ═══ -->
    <section class="new-meeting-card surface-card">
      <h2>New Meeting</h2>
      <p class="text-secondary text-sm">Audio starts immediately. Transcript segments stream live. The finished meeting is saved automatically.</p>

      <label class="field-group">
        <span class="field-label">Meeting title</span>
        <input
          type="text"
          bind:value={meetingTitle}
          placeholder="Quarterly planning sync"
          class="field-input"
          onkeydown={(e) => { if (e.key === "Enter") startMeetingRecording(); }}
        />
        <p class="text-sm text-muted">Leave empty to use the current date.</p>
      </label>

      <button onclick={startMeetingRecording} class="btn btn-primary btn-lg">
        Start Recording
      </button>
    </section>

  {:else if isLoadingMeeting}
    <div class="empty-state">
      <span class="spinner" aria-hidden="true"></span>
      <p class="text-sm text-muted">Loading meeting...</p>
    </div>

  {:else if meeting}
    <!-- ═══ Meeting Item Page ═══ -->
    <div class="meeting-header">
      <div class="meeting-header-left">
        {#if isRecordingMeeting}
          <span class="pill pill-danger recording-pill">
            <span class="recording-dot"></span> Recording
          </span>
        {:else}
          <button onclick={() => { app.currentMeetingId = null; app.currentView = "meeting-history"; }} class="btn btn-ghost btn-back">
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" width="16" height="16">
              <path fill-rule="evenodd" d="M17 10a.75.75 0 0 1-.75.75H5.612l4.158 3.96a.75.75 0 1 1-1.04 1.08l-5.5-5.25a.75.75 0 0 1 0-1.08l5.5-5.25a.75.75 0 1 1 1.04 1.08L5.612 9.25H16.25A.75.75 0 0 1 17 10Z" clip-rule="evenodd" />
            </svg>
            Back to history
          </button>
        {/if}
        <h2>{meeting.title}</h2>
        <div class="meeting-meta">
          {#if !isRecordingMeeting}
            <span class="text-sm text-muted">{formatDate(meeting.started_at)}</span>
            <span class="pill">{formatDuration(meeting.duration_seconds)}</span>
          {/if}
          <span class="text-sm text-muted">{displaySegments.length} segments</span>
        </div>
      </div>

      <div class="meeting-header-actions">
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

    <!-- ═══ Key Insights (shown after recording, if summary exists) ═══ -->
    {#if !isRecordingMeeting && meeting.summary}
      {@const lines = meeting.summary.split("\n").filter((l) => l.trim())}
      {@const keyPoints = lines.filter((l) => /^[-•*]\s/.test(l.trim()) || /^\d+[.)]\s/.test(l.trim())).slice(0, 4)}
      {#if keyPoints.length > 0}
        <div class="insights-grid">
          {#each keyPoints as point, i}
            <div class="insight-card">
              <span class="insight-number">{i + 1}</span>
              <p class="insight-text">{point.replace(/^[-•*\d.)]+\s*/, "").trim()}</p>
            </div>
          {/each}
        </div>
      {/if}
    {/if}

    <!-- ═══ Two-column: Transcript + Summary ═══ -->
    <div class="two-col">
      <!-- Left: Transcript -->
      <section class="surface-card col-card">
        <div class="section-row">
          <h3>Transcript</h3>
          {#if !isRecordingMeeting && displayParagraphs.length > 0}
            <button
              onclick={() => {
                const text = displayParagraphs.map((p) => `[${p.timestamp}] ${p.text}`).join("\n\n");
                navigator.clipboard.writeText(text);
              }}
              class="btn"
            >
              Copy
            </button>
          {/if}
        </div>
        <div class="transcript-output">
          {#if displayParagraphs.length > 0}
            {#each displayParagraphs as para}
              <p class="paragraph">
                <span class="timestamp">[{para.timestamp}]</span>
                {para.text}
              </p>
            {/each}
          {:else}
            <div class="transcript-empty">
              {#if isRecordingMeeting}
                <span class="text-muted">Listening for speech...</span>
              {:else}
                <span class="text-muted">No transcript available.</span>
              {/if}
            </div>
          {/if}
        </div>
      </section>

      <!-- Right: Summary -->
      <section class="surface-card col-card">
        <div class="section-row">
          <h3>Summary</h3>
          {#if meeting.summary && !isRecordingMeeting}
            <button
              onclick={() => navigator.clipboard.writeText(meeting?.summary || "")}
              class="btn"
            >
              Copy
            </button>
          {/if}
        </div>

        {#if isRecordingMeeting}
          <div class="empty-state">
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" width="32" height="32" class="empty-icon">
              <path stroke-linecap="round" stroke-linejoin="round" d="M12 6v6h4.5m4.5 0a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
            </svg>
            <p class="text-sm text-muted">Stop the recording to generate a summary.</p>
          </div>
        {:else if meeting.summary}
          <div class="summary-output">{meeting.summary}</div>
          {#if meeting.summary_model}
            <span class="pill pill-muted" style="align-self: flex-start;">Generated with {meeting.summary_model}</span>
          {/if}
        {:else}
          <div class="empty-state">
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" width="32" height="32" class="empty-icon">
              <path stroke-linecap="round" stroke-linejoin="round" d="M9.813 15.904 9 18.75l-.813-2.846a4.5 4.5 0 0 0-3.09-3.09L2.25 12l2.846-.813a4.5 4.5 0 0 0 3.09-3.09L9 5.25l.813 2.846a4.5 4.5 0 0 0 3.09 3.09L15.75 12l-2.846.813a4.5 4.5 0 0 0-3.09 3.09ZM18.259 8.715 18 9.75l-.259-1.035a3.375 3.375 0 0 0-2.455-2.456L14.25 6l1.036-.259a3.375 3.375 0 0 0 2.455-2.456L18 2.25l.259 1.035a3.375 3.375 0 0 0 2.455 2.456L21.75 6l-1.036.259a3.375 3.375 0 0 0-2.455 2.456ZM16.894 20.567 16.5 21.75l-.394-1.183a2.25 2.25 0 0 0-1.423-1.423L13.5 18.75l1.183-.394a2.25 2.25 0 0 0 1.423-1.423l.394-1.183.394 1.183a2.25 2.25 0 0 0 1.423 1.423l1.183.394-1.183.394a2.25 2.25 0 0 0-1.423 1.423Z" />
            </svg>
            <p class="text-sm text-muted">No summary yet. Generate one below.</p>
          </div>
        {/if}

        {#if isSummarizing}
          <div class="summary-output streaming">{summaryStream}<span class="cursor">|</span></div>
        {/if}

        <!-- Generate Summary CTA -->
        {#if !isRecordingMeeting && displaySegments.length > 0}
          {#if ollamaAvailable && summaryModels.length > 0}
            <div class="summary-cta">
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
                  <span class="spinner" aria-hidden="true"></span>
                  Generating...
                {:else}
                  {meeting.summary ? "Re-generate Summary" : "Generate Summary"}
                {/if}
              </button>
            </div>
          {:else if !ollamaAvailable}
            <div class="ollama-notice">
              <span class="status-dot"></span>
              <span class="text-sm text-muted">Connect Ollama in Settings to enable summaries.</span>
            </div>
          {/if}
        {/if}
      </section>
    </div>

    <!-- ═══ Footer actions for completed meetings ═══ -->
    {#if !isRecordingMeeting && meeting.id}
      <div class="meeting-footer">
        {#if confirmDelete}
          <span class="text-sm text-muted">Delete this meeting permanently?</span>
          <button onclick={deleteMeeting} class="btn btn-danger">Yes, delete</button>
          <button onclick={() => (confirmDelete = false)} class="btn btn-ghost">Cancel</button>
        {:else}
          <button onclick={() => (confirmDelete = true)} class="btn btn-ghost" style="color: var(--color-danger);">
            Delete meeting
          </button>
        {/if}
      </div>
    {/if}
  {/if}
</div>

<style>
  .view {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  /* ── New Meeting Form ── */
  .new-meeting-card {
    display: flex;
    flex-direction: column;
    gap: 1rem;
    max-width: 480px;
  }

  .field-group {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .btn-lg {
    padding: 0.75rem 1.5rem;
    font-size: 1rem;
  }

  /* ── Meeting Header ── */
  .meeting-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 1rem;
    flex-wrap: wrap;
  }

  .meeting-header-left {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .meeting-header-actions {
    display: flex;
    gap: 0.5rem;
    flex-shrink: 0;
  }

  .meeting-meta {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  .btn-back {
    padding: 0.25rem 0;
    font-size: 0.8125rem;
    gap: 0.25rem;
    margin-bottom: 0.25rem;
  }

  .recording-pill {
    display: inline-flex;
    align-items: center;
    gap: 0.375rem;
  }

  .recording-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--color-danger);
    animation: blink 1.2s ease-in-out infinite;
  }

  @keyframes blink {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.3; }
  }

  /* ── Key Insights Grid ── */
  .insights-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
    gap: 0.75rem;
  }

  .insight-card {
    display: flex;
    gap: 0.75rem;
    padding: 0.875rem;
    background: var(--color-surface-2);
    border-radius: var(--radius-default);
    outline: 1px solid var(--color-ghost-border);
    align-items: flex-start;
  }

  .insight-number {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border-radius: 50%;
    background: color-mix(in srgb, var(--color-accent-blue) 15%, transparent);
    color: var(--color-accent-blue);
    font-size: 0.75rem;
    font-weight: 700;
    flex-shrink: 0;
  }

  .insight-text {
    margin: 0;
    font-size: 0.8125rem;
    color: var(--color-text-secondary);
    line-height: 1.5;
  }

  /* ── Two-column layout ── */
  .two-col {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 1rem;
  }

  .col-card {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .section-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    flex-wrap: wrap;
  }

  /* ── Transcript ── */
  .transcript-output {
    padding: 0.75rem;
    background: var(--color-surface-1);
    border-radius: var(--radius-default);
    outline: 1px solid var(--color-ghost-border);
    color: var(--color-text-secondary);
    min-height: 240px;
    max-height: 480px;
    overflow-y: auto;
    font-size: 0.8125rem;
    line-height: 1.6;
  }

  .transcript-empty {
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 200px;
  }

  .paragraph {
    margin: 0 0 0.75rem 0;
    line-height: 1.65;
  }

  .paragraph:last-child {
    margin-bottom: 0;
  }

  .timestamp {
    color: var(--color-text-muted);
    font-size: 0.7rem;
    margin-right: 0.25rem;
    font-variant-numeric: tabular-nums;
  }

  /* ── Summary ── */
  .summary-output {
    padding: 0.75rem;
    background: var(--color-surface-1);
    border-radius: var(--radius-default);
    outline: 1px solid var(--color-ghost-border);
    color: var(--color-text-secondary);
    white-space: pre-wrap;
    min-height: 100px;
    max-height: 360px;
    overflow-y: auto;
    font-size: 0.8125rem;
    line-height: 1.6;
  }

  .summary-output.streaming {
    min-height: 80px;
  }

  .cursor {
    color: var(--color-text-muted);
    animation: blink 0.8s step-end infinite;
  }

  .summary-cta {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }

  .ollama-notice {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.5rem 0;
  }

  .empty-state {
    padding: 2rem;
    text-align: center;
    color: var(--color-text-muted);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.5rem;
  }

  .empty-icon {
    opacity: 0.4;
  }

  /* ── Footer ── */
  .meeting-footer {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding-top: 0.5rem;
    border-top: 1px solid var(--color-ghost-border);
  }

  /* ── Status / Helpers ── */
  .status-banner {
    padding: 0.75rem 1rem;
    border-radius: var(--radius-default);
    background: var(--color-surface-3);
    outline: 1px solid var(--color-ghost-border);
  }

  .status-banner.warning {
    outline-color: color-mix(in srgb, var(--color-warning) 30%, transparent);
  }

  .text-secondary { color: var(--color-text-secondary); }
  .text-muted { color: var(--color-text-muted); }
  .text-sm { font-size: 0.8125rem; }

  @media (max-width: 700px) {
    .two-col {
      grid-template-columns: 1fr;
    }
    .insights-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
