<script lang="ts">
  import { invoke, Channel } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import type { TranscriptionSegment, ModelStatus, DownloadProgress, DictationEntry } from "../types";
  import { getAppState } from "../stores/app.svelte";
  import { errorMessage, useEventListeners } from "../utils";
  import StatusBanner from "./ui/StatusBanner.svelte";
  import CopyButton from "./ui/CopyButton.svelte";
  import ConfirmAction from "./ui/ConfirmAction.svelte";
  import EmptyState from "./ui/EmptyState.svelte";
  import Spinner from "./ui/Spinner.svelte";

  const app = getAppState();

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

  const events = useEventListeners();

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

    events.add("shortcut-toggle", () => {
      if (!isStartingRecording && !isStopping) toggleRecording(true);
    });

    events.add("shortcut-ptt-start", () => {
      if (!app.isRecording && !isStartingRecording && !isStopping) toggleRecording(true);
    });

    events.add("shortcut-ptt-stop", () => {
      if (app.isRecording && !isStopping) toggleRecording(true);
    });

    return () => events.cleanup();
  });

  async function checkModelStatus() {
    try {
      const status: ModelStatus = await invoke("get_model_status");
      modelDownloaded = status.downloaded;
      modelLoaded = status.loaded;
      engineName = status.engine_name;
    } catch (e) {
      statusMessage = errorMessage(e);
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
      statusMessage = errorMessage(e);
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
      statusMessage = errorMessage(e);
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

    if (app.isRecording) {
      isStopping = true;
      try {
        await invoke("stop_transcription");
        app.isRecording = false;
        app.recordingMode = "idle";

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
              statusMessage = `Paste failed: ${errorMessage(e)}`;
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
        statusMessage = errorMessage(e);
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
      app.isRecording = true;
      app.recordingMode = "dictation";
    } catch (e) {
      statusMessage = errorMessage(e);
    } finally {
      isStartingRecording = false;
    }
  }

</script>

<div class="flex flex-col gap-4">
  <!-- Model gate -->
  {#if !modelDownloaded || !modelLoaded}
    <section class="surface-card">
      <div class="flex items-center justify-between gap-4 flex-wrap">
        <div class="flex-1 min-w-0">
          <h3>{!modelDownloaded ? "Download Kyutai STT" : "Load the dictation model"}</h3>
          <p class="text-text-secondary text-sm">
            {!modelDownloaded
              ? "The speech model takes about 2.4 GB on disk."
              : "The first load takes a few seconds while Metal, the tokenizer, and weights warm up."}
          </p>
        </div>

        {#if !modelDownloaded}
          <button onclick={handleDownloadModel} class="btn btn-primary" disabled={isDownloading}>
            {#if isDownloading}
              <Spinner />
              Downloading...
            {:else}
              Download model
            {/if}
          </button>
        {:else}
          <button onclick={handleLoadModel} class="btn btn-primary" disabled={isLoadingModel}>
            {#if isLoadingModel}
              <Spinner />
              Loading...
            {:else}
              Load model
            {/if}
          </button>
        {/if}
      </div>

      {#if isDownloading || isLoadingModel}
        <div class="rounded-default bg-surface-3 px-4 py-3 outline-1 outline-ghost-border mt-3">
          <strong>{isDownloading ? (downloadFile || "Downloading model files...") : "Preparing engine..."}</strong>
          <p class="text-text-muted text-sm">
            {isDownloading ? "Keep the app open while files download." : "Usually takes a few seconds on first load."}
          </p>
        </div>
      {/if}
    </section>
  {/if}

  {#if statusMessage}
    <StatusBanner message={statusMessage} variant="warning" />
  {/if}

  <!-- Hero area -->
  <section class="surface-card">
    <div class="flex items-center justify-between gap-3 flex-wrap">
      <div class="flex flex-wrap gap-2">
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
  <div class="grid grid-cols-[minmax(280px,1fr)_minmax(280px,1.2fr)] gap-4 max-[700px]:grid-cols-1">
    <section class="surface-card flex flex-col items-center gap-4 text-center">
      <h3>
        {#if isStartingRecording}
          Starting the microphone...
        {:else if app.isRecording}
          Listening now
        {:else}
          Ready when you are
        {/if}
      </h3>
      <p class="text-text-secondary text-sm">
        {#if isStartingRecording}
          Warming up the engine.
        {:else if app.isRecording}
          Speak naturally. Text streams into the panel.
        {:else}
          Tap the button to begin.
        {/if}
      </p>

      <button
        onclick={() => toggleRecording()}
        disabled={!modelLoaded || isLoadingModel || isStartingRecording}
        aria-label={app.isRecording ? "Stop recording" : "Start recording"}
        class="record-button"
        class:is-starting={isStartingRecording}
        class:is-recording={app.isRecording}
      >
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="40" height="40" aria-hidden="true">
          <path d="M12 1a4 4 0 0 0-4 4v7a4 4 0 0 0 8 0V5a4 4 0 0 0-4-4Z" />
          <path d="M6 10a1 1 0 0 0-2 0 8 8 0 0 0 7 7.93V21H8a1 1 0 1 0 0 2h8a1 1 0 1 0 0-2h-3v-3.07A8 8 0 0 0 20 10a1 1 0 1 0-2 0 6 6 0 0 1-12 0Z" />
        </svg>
      </button>

      <span class="text-sm text-text-secondary">
        {#if isStartingRecording}
          Warming up...
        {:else if app.isRecording}
          Tap to stop
        {:else if modelLoaded}
          Tap to start
        {:else}
          Load model first
        {/if}
      </span>

      <div class="flex gap-6 mt-2">
        <div class="flex flex-col gap-0.5 text-center">
          <span class="field-label">Input</span>
          <span class="text-sm">{app.selectedDevice || "Default device"}</span>
        </div>
        <div class="flex flex-col gap-0.5 text-center">
          <span class="field-label">Output</span>
          <span class="text-sm">{app.settings.auto_paste ? "Auto-paste" : "Manual copy"}</span>
        </div>
      </div>
    </section>

    <section class="surface-card flex flex-col gap-3">
      <div class="flex items-center justify-between gap-4 flex-wrap">
        <h3>Transcript</h3>
        {#if transcript}
          <CopyButton text={transcript} />
        {/if}
      </div>

      {#if transcript}
        <div class="p-3 bg-surface-1 rounded-default outline-1 outline-ghost-border text-text-secondary whitespace-pre-wrap min-h-40 max-h-[360px] overflow-y-auto text-sm leading-relaxed">{transcript}</div>
      {:else}
        <EmptyState
          title={isStartingRecording ? "Warming up..." : app.isRecording ? "Listening for speech" : "No transcript yet"}
          message={app.isRecording || isStartingRecording ? "Text will appear as segments arrive." : "Press the mic button to start."}
        />
      {/if}
    </section>
  </div>

  <!-- History -->
  {#if history.length > 0}
    <section class="surface-card">
      <div class="flex items-center justify-between gap-4 flex-wrap">
        <h3>History <span class="text-sm text-text-muted font-normal">({history.length})</span></h3>
        <ConfirmAction
          label="Clear all"
          confirmLabel="Yes, clear"
          confirmMessage="Clear all entries?"
          variant="danger"
          onConfirm={clearHistory}
        />
      </div>

      <div class="flex flex-col gap-2 mt-2">
        {#each history as entry}
          {@const isExpanded = expandedEntryId === entry.id}
          <div class="rounded-default outline-1 outline-ghost-border bg-surface-1 overflow-hidden">
            <button
              onclick={() => (expandedEntryId = isExpanded ? null : entry.id)}
              class="w-full px-3 py-2.5 text-left cursor-pointer transition-colors duration-150 hover:bg-surface-3"
            >
              <span class="text-xs text-text-muted">{new Date(entry.timestamp).toLocaleString()}</span>
              <p class={`mt-1 mb-0 text-text-secondary text-sm leading-normal whitespace-pre-wrap break-words ${!isExpanded ? "line-clamp-5" : ""}`}>{entry.text}</p>
            </button>
            <div class="flex gap-1 px-3 pb-2">
              <button onclick={() => navigator.clipboard.writeText(entry.text)} class="btn btn-ghost btn-sm">Copy</button>
              <button onclick={() => deleteHistoryEntry(entry.id)} class="btn btn-ghost btn-sm text-danger">Delete</button>
            </div>
          </div>
        {/each}
      </div>
    </section>
  {/if}
</div>
