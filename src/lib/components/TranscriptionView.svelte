<script lang="ts">
  import { invoke, Channel } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import type { TranscriptionSegment, ModelStatus, DownloadProgress, DictationEntry } from "../types";
  import { getAppState } from "../stores/app.svelte";

  const app = getAppState();

  let isRecording = $state(false);
  let isStartingRecording = $state(false);
  let isStopping = $state(false);
  let transcript = $state("");
  let engineName = $state("Kyutai STT 1B - FR/EN");
  let statusMessage = $state("");

  let modelDownloaded = $state(false);
  let modelLoaded = $state(false);
  let isDownloading = $state(false);
  let downloadFile = $state("");
  let isLoadingModel = $state(false);

  let history = $state<DictationEntry[]>([]);
  let expandedEntryId = $state<string | null>(null);
  let confirmClearAll = $state(false);

  let cleanupFns: (() => void)[] = [];

  async function loadHistory() {
    try {
      history = await invoke("list_dictation_entries", { limit: 50 });
    } catch {
      // First run.
    }
  }

  async function addHistoryEntry(text: string) {
    if (!text.trim()) return;
    try {
      await invoke("add_dictation_entry", { text: text.trim() });
      await loadHistory();
    } catch (e) {
      console.warn("Failed to save dictation entry:", e);
    }
  }

  async function deleteHistoryEntry(id: string) {
    try {
      await invoke("delete_dictation_entry", { id });
      if (expandedEntryId === id) expandedEntryId = null;
      await loadHistory();
    } catch (e) {
      console.warn("Failed to delete dictation entry:", e);
    }
  }

  async function clearHistory() {
    try {
      await invoke("clear_dictation_history");
      history = [];
      expandedEntryId = null;
    } catch (e) {
      console.warn("Failed to clear history:", e);
    }
  }

  onMount(() => {
    checkModelStatus();
    loadHistory();

    listen("shortcut-toggle", () => {
      if (!isStartingRecording && !isStopping) toggleRecording(true);
    }).then((fn) => cleanupFns.push(fn));

    listen("shortcut-ptt-start", () => {
      if (!isRecording && !isStartingRecording && !isStopping) toggleRecording(true);
    }).then((fn) => cleanupFns.push(fn));

    listen("shortcut-ptt-stop", () => {
      if (isRecording && !isStopping) toggleRecording(true);
    }).then((fn) => cleanupFns.push(fn));

    return () => {
      cleanupFns.forEach((fn) => fn());
    };
  });

  async function checkModelStatus() {
    try {
      const status: ModelStatus = await invoke("get_model_status");
      modelDownloaded = status.downloaded;
      modelLoaded = status.loaded;
      engineName = status.engine_name;
    } catch (e) {
      statusMessage = String(e);
    }
  }

  async function handleDownloadModel() {
    isDownloading = true;
    statusMessage = "";
    downloadFile = "";

    try {
      const channel = new Channel<DownloadProgress>();
      channel.onmessage = (progress) => {
        downloadFile = progress.file;
        if (typeof progress.status === "object" && "error" in progress.status) {
          statusMessage = `Download error: ${progress.status.error}`;
          isDownloading = false;
        } else if (progress.status === "complete" && progress.file === "all") {
          isDownloading = false;
          modelDownloaded = true;
          downloadFile = "";
        } else if (progress.status === "complete") {
          downloadFile = `${progress.file} done`;
        }
      };

      await invoke("download_model", { channel });
      await checkModelStatus();
    } catch (e) {
      statusMessage = String(e);
      isDownloading = false;
    }
  }

  async function handleLoadModel() {
    isLoadingModel = true;
    statusMessage = "";
    try {
      await invoke("load_model");
      modelLoaded = true;
      await checkModelStatus();
    } catch (e) {
      statusMessage = String(e);
    } finally {
      isLoadingModel = false;
    }
  }

  /**
   * @param fromShortcut — true when triggered by global shortcut (auto-paste allowed),
   *                        false when triggered by the UI button (clipboard only)
   */
  async function toggleRecording(fromShortcut = false) {
    if (isStartingRecording || isStopping) return;

    if (isRecording) {
      isStopping = true;
      try {
        await invoke("stop_transcription");
        isRecording = false;

        await addHistoryEntry(transcript);

        if (transcript.trim()) {
          if (fromShortcut && app.settings.auto_paste) {
            // Shortcut: copy + simulate Cmd+V
            try {
              await invoke("paste_text", {
                text: transcript.trim(),
                delayMs: app.settings.paste_delay_ms,
              });
            } catch (e) {
              statusMessage = `Paste failed: ${String(e)}`;
            }
          } else {
            // Button: copy to clipboard only
            try {
              await navigator.clipboard.writeText(transcript.trim());
            } catch {
              // Clipboard API may fail silently in some contexts
            }
          }
        }
      } catch (e) {
        statusMessage = String(e);
      } finally {
        isStopping = false;
      }
      return;
    }

    if (!modelLoaded) {
      statusMessage = modelDownloaded
        ? "Load the model before starting dictation."
        : "Download and load the model before starting dictation.";
      return;
    }

    transcript = "";
    statusMessage = "";
    isStartingRecording = true;

    try {
      let lastTime = 0;
      const pauseThreshold = 1.5;

      const channel = new Channel<TranscriptionSegment>();
      channel.onmessage = (segment) => {
        if (segment.is_final) {
          if (transcript) {
            const gap = segment.start_time - lastTime;
            const endsWithSentence = /[.!?…]\s*$/.test(transcript);
            if (gap >= pauseThreshold && endsWithSentence && !transcript.endsWith("\n")) {
              transcript += "\n\n";
            } else if (!transcript.endsWith(" ") && !transcript.endsWith("\n") && !segment.text.startsWith(" ")) {
              transcript += " ";
            }
          }
          transcript += segment.text;
          lastTime = segment.start_time;
        }
      };

      await invoke("start_transcription", { channel });
      isRecording = true;
    } catch (e) {
      statusMessage = String(e);
    } finally {
      isStartingRecording = false;
    }
  }

  export function getRecordingState() {
    return isRecording;
  }
</script>

<div class="view">
  <!-- Model gate -->
  {#if !modelDownloaded || !modelLoaded}
    <section class="surface-card">
      <div class="section-row">
        <div class="section-text">
          <h3>{!modelDownloaded ? "Download Kyutai STT" : "Load the dictation model"}</h3>
          <p class="text-secondary text-sm">
            {!modelDownloaded
              ? "The speech model takes about 2.4 GB on disk."
              : "The first load takes a few seconds while Metal, the tokenizer, and weights warm up."}
          </p>
        </div>

        {#if !modelDownloaded}
          <button onclick={handleDownloadModel} class="btn btn-primary" disabled={isDownloading}>
            {#if isDownloading}
              <span class="spinner" aria-hidden="true"></span>
              Downloading...
            {:else}
              Download model
            {/if}
          </button>
        {:else}
          <button onclick={handleLoadModel} class="btn btn-primary" disabled={isLoadingModel}>
            {#if isLoadingModel}
              <span class="spinner" aria-hidden="true"></span>
              Loading...
            {:else}
              Load model
            {/if}
          </button>
        {/if}
      </div>

      {#if isDownloading || isLoadingModel}
        <div class="status-banner">
          <strong>{isDownloading ? (downloadFile || "Downloading model files...") : "Preparing engine..."}</strong>
          <p class="text-muted text-sm">
            {isDownloading ? "Keep the app open while files download." : "Usually takes a few seconds on first load."}
          </p>
        </div>
      {/if}
    </section>
  {/if}

  {#if statusMessage}
    <div class="status-banner warning">
      <p class="text-sm">{statusMessage}</p>
    </div>
  {/if}

  <!-- Hero area -->
  <section class="surface-card">
    <div class="hero-row">
      <div class="hero-badges">
        <span class="pill pill-blue">{engineName}</span>
        <span class={`pill ${modelLoaded ? "pill-success" : modelDownloaded ? "pill-warning" : ""}`}>
          {modelLoaded ? "Model ready" : modelDownloaded ? "Load required" : "Download required"}
        </span>
        <span class={`pill ${app.settings.auto_paste ? "pill-blue" : "pill-muted"}`}>
          {app.settings.auto_paste ? `Auto-paste ${app.settings.paste_delay_ms}ms` : "Manual copy"}
        </span>
      </div>
    </div>
  </section>

  <!-- Record + Transcript -->
  <div class="two-col">
    <section class="surface-card center-col">
      <h3 class="text-center">
        {#if isStartingRecording}
          Starting the microphone...
        {:else if isRecording}
          Listening now
        {:else}
          Ready when you are
        {/if}
      </h3>
      <p class="text-secondary text-sm text-center">
        {#if isStartingRecording}
          Warming up the engine.
        {:else if isRecording}
          Speak naturally. Text streams into the panel.
        {:else}
          Tap the button to begin.
        {/if}
      </p>

      <button
        onclick={toggleRecording}
        disabled={!modelLoaded || isLoadingModel || isStartingRecording}
        aria-label={isRecording ? "Stop recording" : "Start recording"}
        class="record-button"
        class:is-starting={isStartingRecording}
        class:is-recording={isRecording}
      >
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="40" height="40" aria-hidden="true">
          <path d="M12 1a4 4 0 0 0-4 4v7a4 4 0 0 0 8 0V5a4 4 0 0 0-4-4Z" />
          <path d="M6 10a1 1 0 0 0-2 0 8 8 0 0 0 7 7.93V21H8a1 1 0 1 0 0 2h8a1 1 0 1 0 0-2h-3v-3.07A8 8 0 0 0 20 10a1 1 0 1 0-2 0 6 6 0 0 1-12 0Z" />
        </svg>
      </button>

      <span class="text-sm text-secondary">
        {#if isStartingRecording}
          Warming up...
        {:else if isRecording}
          Tap to stop
        {:else if modelLoaded}
          Tap to start
        {:else}
          Load model first
        {/if}
      </span>

      <div class="metrics-row">
        <div class="metric">
          <span class="field-label">Input</span>
          <span class="text-sm">{app.selectedDevice || "Default device"}</span>
        </div>
        <div class="metric">
          <span class="field-label">Output</span>
          <span class="text-sm">{app.settings.auto_paste ? "Auto-paste" : "Manual copy"}</span>
        </div>
      </div>
    </section>

    <section class="surface-card transcript-col">
      <div class="section-row">
        <h3>Transcript</h3>
        {#if transcript}
          <button onclick={() => navigator.clipboard.writeText(transcript)} class="btn">Copy</button>
        {/if}
      </div>

      {#if transcript}
        <div class="transcript-output">{transcript}</div>
      {:else}
        <div class="empty-state">
          <strong>
            {#if isStartingRecording}
              Warming up...
            {:else if isRecording}
              Listening for speech
            {:else}
              No transcript yet
            {/if}
          </strong>
          <p class="text-sm text-muted">
            {#if isRecording || isStartingRecording}
              Text will appear as segments arrive.
            {:else}
              Press the mic button to start.
            {/if}
          </p>
        </div>
      {/if}
    </section>
  </div>

  <!-- History -->
  {#if history.length > 0}
    <section class="surface-card">
      <div class="section-row">
        <h3>History <span class="text-sm text-muted" style="font-weight: 400;">({history.length})</span></h3>
        {#if confirmClearAll}
          <div class="btn-group">
            <span class="text-sm text-muted">Clear all entries?</span>
            <button onclick={() => { clearHistory(); confirmClearAll = false; }} class="btn btn-danger">Yes, clear</button>
            <button onclick={() => (confirmClearAll = false)} class="btn btn-ghost">Cancel</button>
          </div>
        {:else}
          <button onclick={() => (confirmClearAll = true)} class="btn btn-ghost" style="color: var(--color-danger);">Clear all</button>
        {/if}
      </div>

      <div class="history-list">
        {#each history as entry}
          {@const isExpanded = expandedEntryId === entry.id}
          <div class="history-item">
            <button
              onclick={() => (expandedEntryId = isExpanded ? null : entry.id)}
              class="history-content"
              class:is-expanded={isExpanded}
            >
              <span class="history-date">{new Date(entry.timestamp).toLocaleString()}</span>
              <p class="history-text" class:is-clamped={!isExpanded}>{entry.text}</p>
            </button>
            <div class="history-actions">
              <button onclick={() => navigator.clipboard.writeText(entry.text)} class="btn btn-ghost btn-sm">Copy</button>
              <button onclick={() => deleteHistoryEntry(entry.id)} class="btn btn-ghost btn-sm" style="color: var(--color-danger);">Delete</button>
            </div>
          </div>
        {/each}
      </div>
    </section>
  {/if}
</div>

<style>
  .view {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .section-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    flex-wrap: wrap;
  }

  .section-text {
    flex: 1;
    min-width: 0;
  }

  .hero-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    flex-wrap: wrap;
  }

  .hero-badges {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
  }

  .two-col {
    display: grid;
    grid-template-columns: minmax(280px, 1fr) minmax(280px, 1.2fr);
    gap: 1rem;
  }

  .center-col {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 1rem;
    text-align: center;
  }

  .text-center {
    text-align: center;
  }

  .text-secondary {
    color: var(--color-text-secondary);
  }

  .text-muted {
    color: var(--color-text-muted);
  }

  .text-sm {
    font-size: 0.8125rem;
  }

  .transcript-col {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .transcript-output {
    padding: 0.75rem;
    background: var(--color-surface-1);
    border-radius: var(--radius-default);
    outline: 1px solid var(--color-ghost-border);
    color: var(--color-text-secondary);
    white-space: pre-wrap;
    min-height: 160px;
    max-height: 360px;
    overflow-y: auto;
    font-size: 0.875rem;
    line-height: 1.6;
  }

  .empty-state {
    padding: 1.5rem;
    text-align: center;
    color: var(--color-text-muted);
  }

  .empty-state strong {
    display: block;
    margin-bottom: 0.25rem;
    color: var(--color-text-secondary);
  }

  .status-banner {
    padding: 0.75rem 1rem;
    border-radius: var(--radius-default);
    background: var(--color-surface-3);
    outline: 1px solid var(--color-ghost-border);
  }

  .status-banner.warning {
    outline-color: color-mix(in srgb, var(--color-warning) 30%, transparent);
  }

  .metrics-row {
    display: flex;
    gap: 1.5rem;
    margin-top: 0.5rem;
  }

  .metric {
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
    text-align: center;
  }

  .btn-group {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  .history-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin-top: 0.5rem;
  }

  .history-item {
    border-radius: var(--radius-default);
    outline: 1px solid var(--color-ghost-border);
    background: var(--color-surface-1);
    overflow: hidden;
  }

  .history-content {
    width: 100%;
    padding: 0.625rem 0.75rem;
    text-align: left;
    cursor: pointer;
    transition: background 150ms ease;
  }

  .history-content:hover {
    background: var(--color-surface-3);
  }

  .history-date {
    font-size: 0.7rem;
    color: var(--color-text-muted);
  }

  .history-text {
    margin: 0.25rem 0 0;
    color: var(--color-text-secondary);
    font-size: 0.8125rem;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .history-text.is-clamped {
    display: -webkit-box;
    -webkit-line-clamp: 5;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .history-actions {
    display: flex;
    gap: 0.25rem;
    padding: 0.25rem 0.75rem 0.5rem;
  }

  .btn-sm {
    padding: 0.25rem 0.5rem;
    font-size: 0.75rem;
    min-height: auto;
  }

  @media (max-width: 700px) {
    .two-col {
      grid-template-columns: 1fr;
    }
  }
</style>
