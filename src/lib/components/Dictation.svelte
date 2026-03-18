<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";

  let isRecording = $state(false);
  let transcript = $state("");
  let engineName = $state("Kyutai STT 1B - FR/EN");
  let statusMessage = $state("");

  onMount(() => {
    // Listen for hotkey-triggered recording events from Rust backend
    const unlistenStarted = listen("recording-started", () => {
      isRecording = true;
      statusMessage = "";
    });
    const unlistenStopped = listen("recording-stopped", async () => {
      isRecording = false;
      // Fetch the saved audio result
      try {
        const result: string = await invoke("stop_recording");
        transcript = result;
      } catch {
        // Recording was already stopped by the hotkey handler
      }
    });

    return () => {
      unlistenStarted.then((fn) => fn());
      unlistenStopped.then((fn) => fn());
    };
  });

  async function toggleRecording() {
    try {
      if (isRecording) {
        const result: string = await invoke("stop_recording");
        transcript = result;
        isRecording = false;
      } else {
        transcript = "";
        statusMessage = "";
        await invoke("start_recording");
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

  <button
    onclick={toggleRecording}
    aria-label={isRecording ? "Stop recording" : "Start recording"}
    class="w-24 h-24 rounded-full flex items-center justify-center transition-all duration-200 cursor-pointer
      {isRecording
        ? 'bg-red-500/20 border-2 border-red-500 text-red-400 shadow-lg shadow-red-500/20'
        : 'bg-zinc-800 border-2 border-zinc-700 text-zinc-400 hover:border-zinc-500 hover:text-zinc-200'}"
  >
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" class="w-10 h-10">
      <path d="M12 1a4 4 0 0 0-4 4v7a4 4 0 0 0 8 0V5a4 4 0 0 0-4-4Z" />
      <path d="M6 10a1 1 0 0 0-2 0 8 8 0 0 0 7 7.93V21H8a1 1 0 1 0 0 2h8a1 1 0 1 0 0-2h-3v-3.07A8 8 0 0 0 20 10a1 1 0 1 0-2 0 6 6 0 0 1-12 0Z" />
    </svg>
  </button>

  {#if isRecording}
    <p class="text-sm text-red-400 animate-pulse">Recording...</p>
  {/if}

  <p class="text-xs text-zinc-600">⌘⇧Space to toggle</p>

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
</div>
