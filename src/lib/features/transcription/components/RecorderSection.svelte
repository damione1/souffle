<script lang="ts">
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
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" width="40" height="40" aria-hidden="true">
      <path d="M12 1a4 4 0 0 0-4 4v7a4 4 0 0 0 8 0V5a4 4 0 0 0-4-4Z" />
      <path d="M6 10a1 1 0 0 0-2 0 8 8 0 0 0 7 7.93V21H8a1 1 0 1 0 0 2h8a1 1 0 1 0 0-2h-3v-3.07A8 8 0 0 0 20 10a1 1 0 1 0-2 0 6 6 0 0 1-12 0Z" />
    </svg>
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
