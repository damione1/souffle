<script lang="ts">
  import { invoke, Channel } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import appIcon from "../../../app-icon1.png";
  import type { TranscriptionSegment, ModelStatus, DownloadProgress, DictationEntry } from "../types";
  import { getAppState } from "../stores/app.svelte";

  const app = getAppState();

  let isRecording = $state(false);
  let isStartingRecording = $state(false);
  let transcript = $state("");
  let engineName = $state("Kyutai STT 1B - FR/EN");
  let statusMessage = $state("");

  let modelDownloaded = $state(false);
  let modelLoaded = $state(false);
  let isDownloading = $state(false);
  let downloadFile = $state("");
  let isLoadingModel = $state(false);

  let history = $state<DictationEntry[]>([]);
  let showHistory = $state(false);
  let expandedEntryId = $state<string | null>(null);

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
      if (!isStartingRecording) toggleRecording();
    }).then((fn) => cleanupFns.push(fn));

    listen("shortcut-ptt-start", () => {
      if (!isRecording && !isStartingRecording) toggleRecording();
    }).then((fn) => cleanupFns.push(fn));

    listen("shortcut-ptt-stop", () => {
      if (isRecording) toggleRecording();
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

  async function toggleRecording() {
    if (isStartingRecording) return;

    if (isRecording) {
      try {
        await invoke("stop_transcription");
        isRecording = false;

        await addHistoryEntry(transcript);

        if (app.settings.auto_paste && transcript.trim()) {
          try {
            await invoke("paste_text", {
              text: transcript.trim(),
              delayMs: app.settings.paste_delay_ms,
            });
          } catch (e) {
            statusMessage = `Paste failed: ${String(e)}`;
          }
        }
      } catch (e) {
        statusMessage = String(e);
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
</script>

<div class="view-stack">
  <section class="surface-card surface-card--compact dictation-toolbar">
    <div class="dictation-lockup">
      <img class="dictation-app-icon" src={appIcon} alt="Soufflé app icon" />
      <div class="stack-sm">
        <div class="dictation-heading-row">
          <h2 class="section-title">Soufflé Dictation</h2>
          <span class="pill">{engineName}</span>
        </div>
        <p class="helper-text">Rise gently. Dictate boldly.</p>
      </div>
    </div>

    <div class="action-row" style="flex-wrap: wrap;">
      <span class={`pill ${modelLoaded ? "pill--success" : modelDownloaded ? "pill--warning" : ""}`}>
        {modelLoaded ? "Model ready" : modelDownloaded ? "Load required" : "Download required"}
      </span>
      <span class={`pill ${app.settings.auto_paste ? "pill--primary" : ""}`}>
        {app.settings.auto_paste ? `Auto-paste ${app.settings.paste_delay_ms}ms` : "Manual copy"}
      </span>
      <span class="pill">Shortcuts in Settings</span>
    </div>
  </section>

  {#if !modelDownloaded || !modelLoaded}
    <section class="surface-card surface-card--compact stack-md">
      <div class="section-header">
        <div>
          <p class="eyebrow">Model</p>
          <h3 class="section-title">{!modelDownloaded ? "Download Kyutai STT" : "Load the dictation model"}</h3>
          <p class="section-description">
            {!modelDownloaded
              ? "The speech model takes about 2.4 GB on disk."
              : "The first load takes a few seconds while Metal, the tokenizer, and weights warm up."}
          </p>
        </div>

        {#if !modelDownloaded}
          <button onclick={handleDownloadModel} class="button button-primary" disabled={isDownloading}>
            {#if isDownloading}
              <span class="button-spinner" aria-hidden="true"></span>
              Downloading...
            {:else}
              Download model
            {/if}
          </button>
        {:else}
          <button onclick={handleLoadModel} class="button button-primary" disabled={isLoadingModel}>
            {#if isLoadingModel}
              <span class="button-spinner" aria-hidden="true"></span>
              Loading model...
            {:else}
              Load model
            {/if}
          </button>
        {/if}
      </div>

      {#if isDownloading || isLoadingModel}
        <div class="status-banner status-banner--warning">
          <strong>{isDownloading ? (downloadFile || "Downloading model files...") : "Preparing the transcription engine..."}</strong>
          <p class="helper-text">
            {isDownloading
              ? "Keep the app open while the files arrive and unpack."
              : "This usually takes around three seconds on first load."}
          </p>
        </div>
      {/if}
    </section>
  {/if}

  {#if statusMessage}
    <div class="status-banner status-banner--warning">
      <strong>Status</strong>
      <p class="helper-text">{statusMessage}</p>
    </div>
  {/if}

  <div class="split-grid">
    <section class="surface-card dictation-control-card">
      <div class="panel-header">
        <div>
          <p class="eyebrow">Capture</p>
          <h3 class="section-title">
            {#if isStartingRecording}
              Starting the microphone...
            {:else if isRecording}
              Listening now
            {:else}
              Ready when you are
            {/if}
          </h3>
          <p class="section-description">
            {#if isStartingRecording}
              The engine needs a short warm-up before text starts appearing.
            {:else if isRecording}
              Speak normally. Finalized text will stream into the transcript panel.
            {:else}
              One control, one transcript, zero ceremony.
            {/if}
          </p>
        </div>
        <span class={`pill ${isRecording ? "pill--danger" : isStartingRecording ? "pill--warning" : modelLoaded ? "pill--success" : ""}`}>
          {isRecording ? "Live" : isStartingRecording ? "Starting" : modelLoaded ? "Ready" : "Locked"}
        </span>
      </div>

      <div class="record-stage record-stage--spotlight">
        <button
          onclick={toggleRecording}
          disabled={!modelLoaded || isLoadingModel || isStartingRecording}
          aria-label={isRecording ? "Stop recording" : "Start recording"}
          class="record-button"
          class:is-ready={modelLoaded && !isRecording}
          class:is-starting={isStartingRecording}
          class:is-recording={isRecording}
        >
          <span class="record-orbit" aria-hidden="true"></span>
          <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="54" height="54" aria-hidden="true">
            <path d="M12 1a4 4 0 0 0-4 4v7a4 4 0 0 0 8 0V5a4 4 0 0 0-4-4Z" />
            <path d="M6 10a1 1 0 0 0-2 0 8 8 0 0 0 7 7.93V21H8a1 1 0 1 0 0 2h8a1 1 0 1 0 0-2h-3v-3.07A8 8 0 0 0 20 10a1 1 0 1 0-2 0 6 6 0 0 1-12 0Z" />
          </svg>
        </button>

        <div class="record-state">
          {#if isStartingRecording}
            Warming up...
          {:else if isRecording}
            Tap to stop
          {:else if modelLoaded}
            Tap to start
          {:else}
            Load model first
          {/if}
        </div>

        <p class="record-caption">
          {#if isStartingRecording}
            Starting takes about 1.5 seconds, which is still faster than baking the actual soufflé.
          {:else if isRecording}
            The button glows while capture is active so it stays obvious at a glance.
          {:else if modelLoaded}
            Streaming text will appear in the transcript panel as soon as final segments arrive.
          {:else}
            Use the model control above, then this button becomes available.
          {/if}
        </p>
      </div>

      <div class="metric-grid">
        <article class="metric-card">
          <span class="metric-label">Input flow</span>
          <span class="metric-value">{app.selectedDevice || "System default device"}</span>
        </article>
        <article class="metric-card">
          <span class="metric-label">Output</span>
          <span class="metric-value">{app.settings.auto_paste ? "Auto-paste enabled" : "Copy manually"}</span>
        </article>
      </div>
    </section>

    <section class="surface-card stack-md">
      <div class="panel-header">
        <div>
          <p class="eyebrow">Transcript</p>
          <h3 class="section-title">Latest output</h3>
        </div>
        {#if transcript}
          <button onclick={() => navigator.clipboard.writeText(transcript)} class="button button-secondary">Copy last</button>
        {/if}
      </div>

      {#if transcript}
        <div class="transcript-output">{transcript}</div>
      {:else}
        <div class="empty-state">
          <strong>
            {#if isStartingRecording}
              Warming up the engine
            {:else if isRecording}
              Listening for speech
            {:else}
              No transcript yet
            {/if}
          </strong>
          {#if isStartingRecording}
            The session is starting. Text will appear here as soon as finalized segments come back.
          {:else if isRecording}
            Start speaking and the transcript will populate here with spacing and paragraph breaks applied.
          {:else}
            Press the mic button to start a new dictation.
          {/if}
        </div>
      {/if}
    </section>
  </div>

  <section class="surface-card surface-card--compact stack-md">
    <div class="section-header">
      <div>
        <p class="eyebrow">History</p>
        <h3 class="section-title">Recent entries</h3>
      </div>

      <div class="action-row" style="flex-wrap: wrap;">
        <span class="pill">{history.length} saved</span>
        {#if history.length > 0}
          <button onclick={() => showHistory = !showHistory} class="button button-secondary">
            {showHistory ? "Hide" : "Show"}
          </button>
          <button onclick={clearHistory} class="button button-ghost danger-text">Clear</button>
        {/if}
      </div>
    </div>

    {#if history.length === 0}
      <div class="empty-state">
        <strong>No history yet</strong>
        Finished dictations are saved here automatically so you can revisit them later.
      </div>
    {:else if showHistory}
      <div class="history-list">
        {#each history as entry}
          <article class="history-item">
            <button
              onclick={() => expandedEntryId = expandedEntryId === entry.id ? null : entry.id}
              class="history-toggle"
            >
              <div class="list-item-header">
                <div class="stack-sm" style="min-width: 0;">
                  <span class="history-meta">{new Date(entry.timestamp).toLocaleString()}</span>
                  <span class="history-preview text-truncate">{entry.text}</span>
                </div>
                <span class="pill">{expandedEntryId === entry.id ? "Open" : "Preview"}</span>
              </div>
            </button>

            {#if expandedEntryId === entry.id}
              <div class="history-body stack-md">
                <div class="transcript-output scroll-block">{entry.text}</div>
                <div class="action-row">
                  <button onclick={() => navigator.clipboard.writeText(entry.text)} class="button button-secondary">Copy entry</button>
                </div>
              </div>
            {/if}
          </article>
        {/each}
      </div>
    {:else}
      <div class="status-banner">
        <strong>History is collapsed.</strong>
        <p class="helper-text">Open it when you want to review or copy previous dictations.</p>
      </div>
    {/if}
  </section>
</div>
