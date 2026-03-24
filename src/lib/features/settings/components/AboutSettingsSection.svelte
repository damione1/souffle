<script lang="ts">
  let {
    selectedTranscriptionLabel,
    selectedOllamaModelLabel,
  }: {
    selectedTranscriptionLabel: string;
    selectedOllamaModelLabel: string;
  } = $props();
</script>

<section class="surface-card flex flex-col gap-3.5">
  <h3>About</h3>

  <div class="flex flex-col gap-2">
    <div class="flex justify-between gap-4">
      <span class="text-text-muted text-sm">Version</span>
      <span class="text-sm">v0.1.0</span>
    </div>
    <div class="flex justify-between gap-4">
      <span class="text-text-muted text-sm">Transcription</span>
      <span class="text-sm text-right">{selectedTranscriptionLabel}</span>
    </div>
    <div class="flex justify-between gap-4">
      <span class="text-text-muted text-sm">Summaries</span>
      <span class="text-sm text-right">{selectedOllamaModelLabel || "Ollama not selected"}</span>
    </div>
    <div class="flex justify-between gap-4">
      <span class="text-text-muted text-sm">Privacy</span>
      <span class="text-sm">Everything runs locally.</span>
    </div>
  </div>
</section>
