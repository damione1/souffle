<script lang="ts">
  import { Mic } from "@lucide/svelte";

  let {
    isStartingRecording,
    isRecording,
    modelLoaded,
    isLoadingModel,
    inputDevice,
    autoPaste,
    onToggleRecording,
  }: {
    isStartingRecording: boolean;
    isRecording: boolean;
    modelLoaded: boolean;
    isLoadingModel: boolean;
    inputDevice: string;
    autoPaste: boolean;
    onToggleRecording: () => void | Promise<void>;
  } = $props();
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
    {:else}
      Tap the button to begin.
    {/if}
  </p>

  <button
    onclick={onToggleRecording}
    disabled={!modelLoaded || isLoadingModel || isStartingRecording}
    aria-label={isRecording ? "Stop recording" : "Start recording"}
    class="record-button"
    class:is-starting={isStartingRecording}
    class:is-recording={isRecording}
  >
    <Mic size={40} aria-hidden="true" />
  </button>

  <span class="text-sm text-text-secondary">
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
