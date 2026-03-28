<script lang="ts">
  import { ArrowDownToLine, Cpu, Mic } from "@lucide/svelte";
  import ProgressBar from "../../../components/ui/ProgressBar.svelte";
  import type { TranscriptionRuntimePhase } from "../../../types";
  import type { TranscriptionModelOperationState } from "../state";

  type RecorderPhase =
    | "locked_by_meeting"
    | "starting"
    | "recording"
    | "downloading"
    | "needs_download"
    | "loading"
    | "needs_load"
    | "ready";

  let {
    isStartingRecording,
    isRecording,
    lockedByMeeting,
    runtimePhase,
    modelOperationState,
    downloadFile,
    downloadCompletedFiles,
    downloadTotalFiles,
    inputDevice,
    autoPaste,
    onDownloadModel,
    onLoadModel,
    onToggleRecording,
  }: {
    isStartingRecording: boolean;
    isRecording: boolean;
    lockedByMeeting: boolean;
    runtimePhase: TranscriptionRuntimePhase;
    modelOperationState: TranscriptionModelOperationState;
    downloadFile: string;
    downloadCompletedFiles: number;
    downloadTotalFiles: number;
    inputDevice: string;
    autoPaste: boolean;
    onDownloadModel: () => void | Promise<void>;
    onLoadModel: () => void | Promise<void>;
    onToggleRecording: () => void | Promise<void>;
  } = $props();

  let phase = $derived.by((): RecorderPhase => {
    if (lockedByMeeting) return "locked_by_meeting";
    if (isStartingRecording) return "starting";
    if (isRecording) return "recording";
    if (modelOperationState === "downloading") return "downloading";
    if (runtimePhase === "download_required") return "needs_download";
    if (modelOperationState === "loading") return "loading";
    if (runtimePhase !== "ready") return "needs_load";
    return "ready";
  });

  const headings: Record<RecorderPhase, string> = {
    locked_by_meeting: "Meeting in progress",
    starting: "Starting the microphone...",
    recording: "Listening now",
    downloading: "Ready when you are",
    needs_download: "Ready when you are",
    loading: "Ready when you are",
    needs_load: "Ready when you are",
    ready: "Ready when you are",
  };

  const descriptions: Record<RecorderPhase, string> = {
    locked_by_meeting: "Stop the meeting recording before starting dictation.",
    starting: "Starting up...",
    recording: "Speak naturally. Text appears as you talk.",
    downloading: "Downloading in the background. You can keep using the app.",
    needs_download: "Download the model to start using dictation.",
    loading: "Loading model...",
    needs_load: "Load the model to start recording.",
    ready: "Tap the button to begin.",
  };

  const actionLabels: Record<RecorderPhase, string> = {
    locked_by_meeting: "Meeting recording in progress",
    starting: "Warming up...",
    recording: "Tap to stop",
    downloading: "Downloading model",
    needs_download: "Download model",
    loading: "Loading model...",
    needs_load: "Load model",
    ready: "Tap to start",
  };
</script>

<section class="surface-card flex flex-col items-center gap-4 text-center">
  <h3>{headings[phase]}</h3>
  <p class="text-text-secondary text-sm">{descriptions[phase]}</p>

  {#if phase === "downloading"}
    <button disabled aria-label="Downloading model" class="record-button is-starting">
      <ArrowDownToLine size={40} aria-hidden="true" />
    </button>
  {:else if phase === "needs_download"}
    <button onclick={onDownloadModel} aria-label="Download model" class="record-button">
      <ArrowDownToLine size={40} aria-hidden="true" />
    </button>
  {:else if phase === "loading" || phase === "needs_load"}
    <button
      onclick={onLoadModel}
      disabled={phase === "loading"}
      aria-label="Load model"
      class="record-button"
      class:is-starting={phase === "loading"}
    >
      <Cpu size={40} aria-hidden="true" />
    </button>
  {:else}
    <button
      onclick={onToggleRecording}
      disabled={phase === "starting" || phase === "locked_by_meeting"}
      aria-label={phase === "recording" ? "Stop recording" : "Start recording"}
      class="record-button"
      class:is-starting={phase === "starting"}
      class:is-recording={phase === "recording"}
    >
      <Mic size={40} aria-hidden="true" />
    </button>
  {/if}

  <span class="text-sm text-text-secondary">{actionLabels[phase]}</span>

  {#if phase === "downloading"}
    <div class="w-full max-w-xs">
      <ProgressBar
        value={downloadCompletedFiles}
        max={downloadTotalFiles || 1}
        label={downloadFile || "Preparing download"}
      />
    </div>
  {/if}

  <div class="flex gap-6 mt-2">
    <div class="flex flex-col gap-0.5 text-center">
      <span class="field-label">Input</span>
      <span class="text-sm">{inputDevice || "Default device"}</span>
    </div>
    <div class="flex flex-col gap-0.5 text-center">
      <span class="field-label">Output</span>
      <span class="text-sm">{autoPaste ? "Auto-paste" : "Manual copy"}</span>
    </div>
  </div>
</section>
