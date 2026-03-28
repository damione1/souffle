<script lang="ts">
  import { ArrowDownToLine, Cpu, Mic } from "@lucide/svelte";
  import ProgressBar from "../../../components/ui/ProgressBar.svelte";
  import type { TranscriptionRuntimePhase } from "../../../types";
  import {
    runtimePhaseIsReady,
    runtimePhaseRequiresDownload,
    type TranscriptionModelOperationState,
  } from "../state";

  let {
    isStartingRecording,
    isRecording,
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

  let isDownloadingModel = $derived(modelOperationState === "downloading");
  let isLoadingModel = $derived(modelOperationState === "loading");
  let requiresDownload = $derived(runtimePhaseRequiresDownload(runtimePhase));
  let isReady = $derived(runtimePhaseIsReady(runtimePhase));

  let actionLabel = $derived.by(() => {
    if (isStartingRecording) return "Warming up...";
    if (isRecording) return "Tap to stop";
    if (isDownloadingModel) return "Downloading model";
    if (requiresDownload) return "Download model";
    if (isLoadingModel) return "Preparing engine...";
    if (!isReady) return "Load model";
    return "Tap to start";
  });
</script>

<section class="surface-card flex flex-col items-center gap-4 text-center">
  <h3>
    {#if isStartingRecording}
      Starting the microphone...
    {:else if isRecording}
      Listening now
    {:else}
      Ready when you are
    {/if}
  </h3>
  <p class="text-text-secondary text-sm">
    {#if isStartingRecording}
      Warming up the engine.
    {:else if isRecording}
      Speak naturally. Text streams into the panel.
    {:else if isDownloadingModel}
      The model keeps downloading in the background while you navigate the app.
    {:else if requiresDownload}
      Download the selected model to enable dictation.
    {:else if !isReady}
      Load the selected model into memory before recording.
    {:else}
      Tap the button to begin.
    {/if}
  </p>

  {#if isDownloadingModel}
    <button
      disabled
      aria-label="Downloading model"
      class="record-button"
      class:is-starting={true}
    >
      <ArrowDownToLine size={40} aria-hidden="true" />
    </button>
  {:else if requiresDownload}
    <button
      onclick={onDownloadModel}
      aria-label="Download model"
      class="record-button"
    >
      <ArrowDownToLine size={40} aria-hidden="true" />
    </button>
  {:else if !isReady}
    <button
      onclick={onLoadModel}
      disabled={isLoadingModel}
      aria-label="Load model"
      class="record-button"
      class:is-starting={isLoadingModel}
    >
      <Cpu size={40} aria-hidden="true" />
    </button>
  {:else}
    <button
      onclick={onToggleRecording}
      disabled={isLoadingModel || isStartingRecording}
      aria-label={isRecording ? "Stop recording" : "Start recording"}
      class="record-button"
      class:is-starting={isStartingRecording}
      class:is-recording={isRecording}
    >
      <Mic size={40} aria-hidden="true" />
    </button>
  {/if}

  <span class="text-sm text-text-secondary">
    {actionLabel}
  </span>

  {#if isDownloadingModel}
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
