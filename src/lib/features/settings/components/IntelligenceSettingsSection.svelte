<script lang="ts">
  import StatusBanner from "../../../components/ui/StatusBanner.svelte";
  import type { OllamaModelDescriptor } from "../../../types";

  let {
    ollamaUrl,
    ollamaAvailable,
    ollamaModels,
    summaryModels,
    selectedOllamaModel,
    onOllamaUrlChange,
    onOllamaModelChange,
    onRetryOllama,
  }: {
    ollamaUrl: string;
    ollamaAvailable: boolean;
    ollamaModels: OllamaModelDescriptor[];
    summaryModels: OllamaModelDescriptor[];
    selectedOllamaModel: string;
    onOllamaUrlChange: (event: Event) => void;
    onOllamaModelChange: (event: Event) => void;
    onRetryOllama: () => void | Promise<void>;
  } = $props();
</script>

<section class="surface-card flex flex-col gap-3.5">
  <h3>Summarization</h3>
  <p class="text-text-secondary text-sm">Souffle uses a local Ollama server to generate meeting summaries.</p>

  <div class="flex items-center justify-between gap-4">
    <div>
      <label for="ollama-url" class="block text-[0.9375rem] font-medium text-text-primary">Ollama URL</label>
      <span class="text-sm text-text-muted">Address of your local Ollama server.</span>
    </div>
    <input id="ollama-url" type="text" value={ollamaUrl} onchange={onOllamaUrlChange} class="field-input max-w-64" />
  </div>

  <div class="flex items-center justify-between gap-4">
    <div>
      <span class="block text-[0.9375rem] font-medium text-text-primary">Connection status</span>
      <span class="text-sm text-text-muted">{ollamaModels.length} model{ollamaModels.length === 1 ? "" : "s"} found.</span>
    </div>
    <div class="flex gap-2 items-center">
      <span class="status-dot" class:is-online={ollamaAvailable}></span>
      <span class="text-sm text-text-muted">{ollamaAvailable ? "Connected" : "Not available"}</span>
      <button onclick={onRetryOllama} class="btn">Retry</button>
    </div>
  </div>

  {#if ollamaAvailable && summaryModels.length > 0}
    <div class="flex flex-col gap-1.5">
      <label for="summary-model" class="field-label">Summarization model</label>
      <select
        id="summary-model"
        value={selectedOllamaModel || summaryModels[0].id}
        onchange={onOllamaModelChange}
        class="field-select"
      >
        {#each summaryModels as model}
          <option value={model.id}>{model.label}</option>
        {/each}
      </select>
    </div>
  {:else if ollamaAvailable && ollamaModels.length > 0}
    <StatusBanner message="No compatible model found. Install a model like Llama or Mistral to enable summaries." />
  {/if}
</section>
