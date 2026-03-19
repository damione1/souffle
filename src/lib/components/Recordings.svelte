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

  // Meeting recording state
  let isRecordingMeeting = $state(false);
  let meetingTitle = $state("");
  let liveMeetingTitle = $state("");
  let liveMeetingTranscript = $state("");
  let liveMeetingSegments = $state<TranscriptionSegment[]>([]);
  let liveMeetingLastTime = 0;

  const PAUSE_THRESHOLD = 1.5;

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
          // Refresh the meeting to get saved summary (don't toggle — it's already expanded)
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
          if (gap >= PAUSE_THRESHOLD && endsWithSentence && !liveMeetingTranscript.endsWith("\n")) {
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

<div class="flex flex-col gap-4 w-full max-w-lg">
  <h2 class="text-lg font-semibold text-zinc-100">Recordings</h2>

  <!-- Meeting recording controls -->
  <div class="flex flex-col gap-2 p-3 rounded-lg bg-zinc-900 border border-zinc-800">
    {#if isRecordingMeeting}
      <div class="flex items-center gap-2">
        <div class="w-3 h-3 bg-red-500 rounded-full animate-pulse"></div>
        <span class="text-sm text-red-400">Recording meeting...</span>
      </div>
      <div class="rounded-lg bg-zinc-800/60 border border-zinc-700 p-3">
        <div class="flex items-center justify-between gap-3 mb-2">
          <span class="text-xs font-medium text-zinc-300">{liveMeetingTitle}</span>
          <span class="text-[10px] text-zinc-500">{liveMeetingSegments.length} segments</span>
        </div>
        <div class="text-sm text-zinc-300 whitespace-pre-wrap min-h-20 max-h-52 overflow-y-auto">
          {#if liveMeetingTranscript}
            {liveMeetingTranscript}
          {:else}
            <span class="text-zinc-500 italic">Listening for speech...</span>
          {/if}
        </div>
      </div>
      <button
        onclick={stopMeetingRecording}
        class="px-3 py-1.5 text-sm rounded-lg bg-red-600 hover:bg-red-500 text-white cursor-pointer transition-colors"
      >
        Stop Recording
      </button>
    {:else}
      <input
        type="text"
        bind:value={meetingTitle}
        placeholder="Meeting title (optional)"
        class="px-3 py-1.5 text-sm rounded-lg bg-zinc-800 border border-zinc-700 text-zinc-300
          placeholder:text-zinc-600 focus:border-zinc-500 focus:outline-none"
      />
      <button
        onclick={startMeetingRecording}
        class="px-3 py-1.5 text-sm rounded-lg bg-blue-600 hover:bg-blue-500 text-white cursor-pointer transition-colors"
      >
        Start Meeting Recording
      </button>
      <p class="text-xs text-zinc-600">Records from microphone with live transcription. For system audio, use BlackHole as a virtual audio device.</p>
    {/if}
  </div>

  {#if statusMessage}
    <p class="text-xs text-yellow-500">{statusMessage}</p>
  {/if}

  <!-- Meeting list -->
  {#if meetings.length === 0}
    <p class="text-sm text-zinc-500 text-center py-8">No recordings yet</p>
  {:else}
    <div class="flex flex-col gap-2">
      {#each meetings as meeting}
        <div class="rounded-lg bg-zinc-900 border border-zinc-800 overflow-hidden">
          <!-- Meeting header -->
          <button
            onclick={() => toggleExpand(meeting.id)}
            class="w-full flex items-center justify-between px-4 py-3 cursor-pointer hover:bg-zinc-800/50 transition-colors"
          >
            <div class="flex flex-col items-start gap-0.5">
              <span class="text-sm text-zinc-200">{meeting.title}</span>
              <span class="text-xs text-zinc-500">{formatDate(meeting.started_at)}</span>
            </div>
            <div class="flex items-center gap-3">
              <span class="text-xs text-zinc-500">{formatDuration(meeting.duration_seconds)}</span>
              {#if meeting.has_summary}
                <span class="text-xs text-green-500">Summarized</span>
              {/if}
              <span class="text-zinc-500 text-xs">{expandedId === meeting.id ? "▲" : "▼"}</span>
            </div>
          </button>

          <!-- Expanded content -->
          {#if expandedId === meeting.id && expandedMeeting}
            <div class="border-t border-zinc-800 p-4 flex flex-col gap-3">
              <!-- Transcript -->
              <div>
                <h4 class="text-xs font-medium text-zinc-400 mb-1">Transcript</h4>
                <div class="text-sm text-zinc-300 whitespace-pre-wrap max-h-60 overflow-y-auto p-2 rounded bg-zinc-800/50">
                  {#if expandedMeeting.segments.length > 0}
                    {expandedMeeting.segments.map(s => s.text).join(" ")}
                  {:else}
                    <span class="text-zinc-500 italic">No transcript available</span>
                  {/if}
                </div>
              </div>

              <!-- Summary -->
              {#if expandedMeeting.summary}
                <div>
                  <h4 class="text-xs font-medium text-zinc-400 mb-1">
                    Summary
                    {#if expandedMeeting.summary_model}
                      <span class="text-zinc-600">({expandedMeeting.summary_model})</span>
                    {/if}
                  </h4>
                  <div class="text-sm text-zinc-300 whitespace-pre-wrap p-2 rounded bg-zinc-800/50">
                    {expandedMeeting.summary}
                  </div>
                </div>
              {/if}

              {#if isSummarizing && expandedId === meeting.id}
                <div>
                  <h4 class="text-xs font-medium text-zinc-400 mb-1">
                    {expandedMeeting.summary ? "Generating replacement summary..." : "Generating summary..."}
                  </h4>
                  <div class="text-sm text-zinc-300 whitespace-pre-wrap p-2 rounded bg-zinc-800/50">
                    {summaryStream}<span class="animate-pulse">|</span>
                  </div>
                </div>
              {/if}

              {#if ollamaAvailable && summaryModels.length > 0 && expandedMeeting.segments.length > 0}
                <div class="flex items-center gap-2">
                  <select
                    bind:value={selectedModel}
                    disabled={isSummarizing}
                    class="px-2 py-1 text-xs rounded bg-zinc-800 border border-zinc-700 text-zinc-300 focus:outline-none"
                  >
                    {#each summaryModels as model}
                      <option value={model}>{model}</option>
                    {/each}
                  </select>
                  <button
                    onclick={() => summarizeMeeting(meeting.id)}
                    disabled={isSummarizing}
                    class="px-3 py-1 text-xs rounded bg-purple-600 hover:bg-purple-500 text-white cursor-pointer transition-colors"
                  >
                    {expandedMeeting.summary ? "Re-summarize" : "Summarize"}
                  </button>
                </div>
              {:else if ollamaAvailable && expandedMeeting.segments.length > 0}
                <p class="text-xs text-amber-400">
                  No summary-capable Ollama model detected. Install a text-generation model to generate reliable summaries.
                </p>
              {/if}

              <!-- Actions -->
              <div class="flex items-center gap-2 pt-1">
                <button
                  onclick={() => {
                    const text = expandedMeeting?.segments.map(s => s.text).join(" ") || "";
                    navigator.clipboard.writeText(text);
                  }}
                  class="text-xs text-zinc-500 hover:text-zinc-300 cursor-pointer"
                >
                  Copy transcript
                </button>
                {#if expandedMeeting.summary}
                  <button
                    onclick={() => navigator.clipboard.writeText(expandedMeeting?.summary || "")}
                    class="text-xs text-zinc-500 hover:text-zinc-300 cursor-pointer"
                  >
                    Copy summary
                  </button>
                {/if}
                <button
                  onclick={() => deleteMeeting(meeting.id)}
                  class="text-xs text-red-500/70 hover:text-red-400 cursor-pointer ml-auto"
                >
                  Delete
                </button>
              </div>
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>
