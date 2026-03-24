<script lang="ts">
  let {
    profileLabel,
    modelLoaded,
    modelDownloaded,
    autoPaste,
    pasteDelayMs,
  }: {
    profileLabel: string;
    modelLoaded: boolean;
    modelDownloaded: boolean;
    autoPaste: boolean;
    pasteDelayMs: number;
  } = $props();
</script>

<section class="surface-card">
  <div class="flex items-center justify-between gap-3 flex-wrap">
    <div class="flex flex-wrap gap-2">
      <span class="pill pill-blue">{profileLabel}</span>
      <span class={`pill ${modelLoaded ? "pill-success" : modelDownloaded ? "pill-warning" : "pill-muted"}`}>
        {modelLoaded ? "Model ready" : modelDownloaded ? "Load required" : "Download required"}
      </span>
      <span class={`pill ${autoPaste ? "pill-blue" : "pill-muted"}`}>
        {autoPaste ? `Auto-paste ${pasteDelayMs}ms` : "Manual copy"}
      </span>
    </div>
  </div>
</section>
