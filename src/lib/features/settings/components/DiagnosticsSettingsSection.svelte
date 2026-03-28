<script lang="ts">
  let {
    debugTranscription,
    onDebugTranscriptionChange,
  }: {
    debugTranscription: boolean;
    onDebugTranscriptionChange: (event: Event) => void;
  } = $props();
</script>

<section class="surface-card flex flex-col gap-3.5">
  <h3>Diagnostics</h3>

  <div class="flex items-center justify-between gap-4">
    <div>
      <span class="block text-[0.9375rem] font-medium text-text-primary">Detailed transcription logs</span>
      <span class="text-sm text-text-muted">Log extra details about speech recognition for troubleshooting.</span>
    </div>
    <input
      type="checkbox"
      checked={debugTranscription}
      onchange={onDebugTranscriptionChange}
      class="switch"
      aria-label="Verbose transcription debug logs"
    />
  </div>
</section>
