<script lang="ts">
  import StatusBanner from "../../../components/ui/StatusBanner.svelte";
  import type { OllamaModelDescriptor, TranscriptionCatalog } from "../../../types";
  import {
    getSelectedTranscriptionEngine,
    getSelectedTranscriptionModel,
  } from "../../transcription/catalog";

  let {
    catalog,
    selectedEngineId,
    selectedModelId,
    ollamaUrl,
    ollamaAvailable,
    ollamaModels,
    summaryModels,
    selectedOllamaModel,
    onSelectTranscriptionEngine,
    onSelectTranscriptionModel,
    onOllamaUrlChange,
    onOllamaModelChange,
    onRetryOllama,
  }: {
    catalog: TranscriptionCatalog | null;
    selectedEngineId: string;
    selectedModelId: string;
    ollamaUrl: string;
    ollamaAvailable: boolean;
    ollamaModels: OllamaModelDescriptor[];
    summaryModels: OllamaModelDescriptor[];
    selectedOllamaModel: string;
    onSelectTranscriptionEngine: (engineId: string) => void | Promise<void>;
    onSelectTranscriptionModel: (modelId: string) => void | Promise<void>;
    onOllamaUrlChange: (event: Event) => void;
    onOllamaModelChange: (event: Event) => void;
    onRetryOllama: () => void | Promise<void>;
  } = $props();

  let selectedEngine = $derived(getSelectedTranscriptionEngine(catalog, selectedEngineId));
  let selectedModel = $derived(getSelectedTranscriptionModel(catalog, selectedEngineId, selectedModelId));
  let availableModels = $derived(selectedEngine?.models ?? []);
</script>

<section class="surface-card flex flex-col gap-3.5">
  <h3>Intelligence</h3>
  <p class="text-text-secondary text-sm">Drive transcription and summarization from typed engine and model descriptors.</p>

  <div class="flex flex-col gap-1.5">
    <label for="settings-transcription-engine" class="field-label">Transcription engine</label>
    <select
      id="settings-transcription-engine"
      value={selectedEngineId}
      onchange={(event) => onSelectTranscriptionEngine((event.currentTarget as HTMLSelectElement).value)}
      class="field-select"
      disabled={!catalog}
    >
      {#each catalog?.engines ?? [] as engine}
        <option value={engine.id}>{engine.label}</option>
      {/each}
    </select>
    {#if selectedEngine}
      <p class="text-sm text-text-muted">{selectedEngine.description}</p>
    {/if}
  </div>

  <div class="flex flex-col gap-1.5">
    <label for="settings-transcription-model" class="field-label">Transcription model</label>
    <select
      id="settings-transcription-model"
      value={selectedModelId}
      onchange={(event) => onSelectTranscriptionModel((event.currentTarget as HTMLSelectElement).value)}
      class="field-select"
      disabled={availableModels.length === 0}
    >
      {#each availableModels as model}
        <option value={model.id}>{model.label}</option>
      {/each}
    </select>
    {#if selectedModel}
      <div class="flex gap-2 flex-wrap text-sm text-text-muted">
        <span>{selectedModel.description}</span>
        {#if selectedModel.supported_languages.length > 0}
          <span class="pill pill-muted">Languages: {selectedModel.supported_languages.join(", ")}</span>
        {/if}
      </div>
    {/if}
  </div>

  <div class="flex items-center justify-between gap-4">
    <div>
      <label for="ollama-url" class="block text-[0.9375rem] font-medium text-text-primary">Ollama URL</label>
      <span class="text-sm text-text-muted">Base URL for the current summary provider integration.</span>
    </div>
    <input id="ollama-url" type="text" value={ollamaUrl} onchange={onOllamaUrlChange} class="field-input max-w-64" />
  </div>

  <div class="flex items-center justify-between gap-4">
    <div>
      <span class="block text-[0.9375rem] font-medium text-text-primary">Connection status</span>
      <span class="text-sm text-text-muted">{ollamaModels.length} model descriptor{ollamaModels.length === 1 ? "" : "s"} discovered.</span>
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
    <StatusBanner message="No summary-capable model found. Install a text-generation Ollama model to enable summaries." />
  {/if}
</section>
