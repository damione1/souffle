<script lang="ts">
  import { invoke, Channel } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import type { TranscriptionSegment, ModelStatus, DownloadProgress, DictationEntry } from "../types";
  import { getAppState } from "../stores/app.svelte";

  const app = getAppState();

  let isRecording = $state(false);
  let transcript = $state("");
  let engineName = $state("Kyutai STT 1B - FR/EN");
  let statusMessage = $state("");

  // Model state
  let modelDownloaded = $state(false);
  let modelLoaded = $state(false);
  let isDownloading = $state(false);
  let downloadFile = $state("");
  let isLoadingModel = $state(false);

  // History
  let history = $state<DictationEntry[]>([]);
  let showHistory = $state(false);
  let expandedEntryId = $state<string | null>(null);

  let cleanupFns: (() => void)[] = [];

  async function loadHistory() {
    try {
      history = await invoke("list_dictation_entries", { limit: 50 });
    } catch { /* First run */ }
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
    } catch (e) {
      console.warn("Failed to clear history:", e);
    }
  }

  onMount(() => {
    checkModelStatus();
    loadHistory();

    // Shortcut/tray → toggle transcription (same pipeline as button click)
    listen("shortcut-toggle", () => {
      toggleRecording();
    }).then((fn) => cleanupFns.push(fn));

    // Push-to-talk: start on press, stop on release
    listen("shortcut-ptt-start", () => {
      if (!isRecording) toggleRecording();
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
    try {
      if (isRecording) {
        await invoke("stop_transcription");
        isRecording = false;

        // Save to history before clearing
        await addHistoryEntry(transcript);

        // Auto-paste if enabled and we have text
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
      } else {
        transcript = "";
        statusMessage = "";

        if (modelLoaded) {
          let lastTime = 0;
          const PAUSE_THRESHOLD = 1.5; // seconds

          const channel = new Channel<TranscriptionSegment>();
          channel.onmessage = (segment) => {
            if (segment.is_final) {
              if (transcript) {
                const gap = segment.start_time - lastTime;
                const endsWithSentence = /[.!?…]\s*$/.test(transcript);
                if (gap >= PAUSE_THRESHOLD && endsWithSentence && !transcript.endsWith("\n")) {
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
        } else {
          await invoke("start_recording");
        }
        isRecording = true;
      }
    } catch (e) {
      statusMessage = String(e);
    }
  }
</script>

<div class="flex flex-col items-center gap-6 w-full max-w-lg">
  <h1 class="text-2xl font-semibold text-zinc-100">Souffle</h1>

  <p class="text-xs text-zinc-500">{engineName}</p>

  <!-- Model setup flow -->
  {#if !modelDownloaded}
    <div class="flex flex-col items-center gap-3 p-4 rounded-lg bg-zinc-900 border border-zinc-800 w-full">
      <p class="text-sm text-zinc-400">Model not downloaded yet (~2.4 GB)</p>
      {#if isDownloading}
        <div class="flex items-center gap-2">
          <div class="w-4 h-4 border-2 border-zinc-500 border-t-zinc-200 rounded-full animate-spin"></div>
          <p class="text-xs text-zinc-500">{downloadFile || "Starting download..."}</p>
        </div>
      {:else}
        <button
          onclick={handleDownloadModel}
          class="px-4 py-2 text-sm rounded-lg bg-blue-600 hover:bg-blue-500 text-white cursor-pointer transition-colors"
        >
          Download Kyutai STT
        </button>
      {/if}
    </div>
  {:else if !modelLoaded}
    <div class="flex flex-col items-center gap-3 p-4 rounded-lg bg-zinc-900 border border-zinc-800 w-full">
      <p class="text-sm text-zinc-400">Model downloaded. Load into memory to start.</p>
      {#if isLoadingModel}
        <div class="flex items-center gap-2">
          <div class="w-4 h-4 border-2 border-zinc-500 border-t-zinc-200 rounded-full animate-spin"></div>
          <p class="text-xs text-zinc-500">Loading model (Metal GPU)...</p>
        </div>
      {:else}
        <button
          onclick={handleLoadModel}
          class="px-4 py-2 text-sm rounded-lg bg-green-600 hover:bg-green-500 text-white cursor-pointer transition-colors"
        >
          Load Model
        </button>
      {/if}
    </div>
  {/if}

  <!-- Recording button -->
  <button
    onclick={toggleRecording}
    disabled={!modelLoaded && modelDownloaded}
    aria-label={isRecording ? "Stop recording" : "Start recording"}
    class="w-24 h-24 rounded-full flex items-center justify-center transition-all duration-200
      {isRecording
        ? 'bg-red-500/20 border-2 border-red-500 text-red-400 shadow-lg shadow-red-500/20 cursor-pointer'
        : modelLoaded
          ? 'bg-zinc-800 border-2 border-zinc-700 text-zinc-400 hover:border-zinc-500 hover:text-zinc-200 cursor-pointer'
          : 'bg-zinc-900 border-2 border-zinc-800 text-zinc-600 cursor-not-allowed'}"
  >
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" class="w-10 h-10">
      <path d="M12 1a4 4 0 0 0-4 4v7a4 4 0 0 0 8 0V5a4 4 0 0 0-4-4Z" />
      <path d="M6 10a1 1 0 0 0-2 0 8 8 0 0 0 7 7.93V21H8a1 1 0 1 0 0 2h8a1 1 0 1 0 0-2h-3v-3.07A8 8 0 0 0 20 10a1 1 0 1 0-2 0 6 6 0 0 1-12 0Z" />
    </svg>
  </button>

  {#if isRecording}
    <p class="text-sm text-red-400 animate-pulse">
      {modelLoaded ? "Transcribing..." : "Recording..."}
    </p>
  {/if}

  <div class="flex items-center gap-3">
    <p class="text-xs text-zinc-600">Configure shortcuts in Settings</p>
    {#if app.settings.auto_paste}
      <span class="text-xs text-blue-400">Auto-paste ON</span>
    {/if}
  </div>

  {#if statusMessage}
    <p class="text-xs text-yellow-500">{statusMessage}</p>
  {/if}

  {#if transcript}
    <div class="w-full p-4 rounded-lg bg-zinc-900 border border-zinc-800 text-sm text-zinc-300 whitespace-pre-wrap">
      {transcript}
    </div>
    <button
      onclick={() => navigator.clipboard.writeText(transcript)}
      class="text-xs text-zinc-500 hover:text-zinc-300 cursor-pointer"
    >
      Copy last
    </button>
  {/if}

  <!-- Dictation History -->
  {#if history.length > 0}
    <div class="w-full mt-2">
      <button
        onclick={() => showHistory = !showHistory}
        class="flex items-center gap-2 text-xs text-zinc-500 hover:text-zinc-300 cursor-pointer mb-2"
      >
        <span>{showHistory ? "▲" : "▼"}</span>
        <span>History ({history.length})</span>
      </button>

      {#if showHistory}
        <div class="flex flex-col gap-2">
          {#each history as entry}
            <div class="rounded-lg bg-zinc-900 border border-zinc-800 overflow-hidden">
              <button
                onclick={() => expandedEntryId = expandedEntryId === entry.id ? null : entry.id}
                class="w-full flex items-center justify-between px-3 py-2 cursor-pointer hover:bg-zinc-800/50 transition-colors"
              >
                <div class="flex flex-col items-start gap-0.5 min-w-0">
                  <span class="text-xs text-zinc-500">{new Date(entry.timestamp).toLocaleString()}</span>
                  <span class="text-sm text-zinc-300 truncate w-full text-left">
                    {entry.text.slice(0, 80)}{entry.text.length > 80 ? "..." : ""}
                  </span>
                </div>
                <span class="text-zinc-500 text-xs ml-2 shrink-0">{expandedEntryId === entry.id ? "▲" : "▼"}</span>
              </button>

              {#if expandedEntryId === entry.id}
                <div class="border-t border-zinc-800 p-3">
                  <div class="text-sm text-zinc-300 whitespace-pre-wrap max-h-40 overflow-y-auto mb-2">
                    {entry.text}
                  </div>
                  <button
                    onclick={() => navigator.clipboard.writeText(entry.text)}
                    class="text-xs text-zinc-500 hover:text-zinc-300 cursor-pointer"
                  >
                    Copy
                  </button>
                </div>
              {/if}
            </div>
          {/each}
        </div>

        <button
          onclick={clearHistory}
          class="text-xs text-red-500/70 hover:text-red-400 cursor-pointer mt-2"
        >
          Clear history
        </button>
      {/if}
    </div>
  {/if}
</div>
