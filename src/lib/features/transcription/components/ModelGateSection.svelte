<script lang="ts">
  import Spinner from "../../../components/ui/Spinner.svelte";
  import type { TranscriptionCatalog } from "../../../types";
  import {
    hasAvailableTranscriptionModel,
    isTranscriptionBackendAvailable,
    isTranscriptionModelAvailable,
    getSelectedTranscriptionBackend,
    getSelectedTranscriptionEngine,
    getSelectedTranscriptionModel,
  } from "../catalog";

  let {
    catalog,
    selectedEngineId,
    selectedModelId,
    selectedBackendId,
    modelDownloaded,
    modelLoaded,
    isDownloading,
    downloadFile,
    isLoadingModel,
    onSelectEngine,
    onSelectModel,
    onSelectBackend,
    onDownloadModel,
    onLoadModel,
  }: {
    catalog: TranscriptionCatalog | null;
    selectedEngineId: string;
    selectedModelId: string;
    selectedBackendId: string;
    modelDownloaded: boolean;
    modelLoaded: boolean;
    isDownloading: boolean;
    downloadFile: string;
    isLoadingModel: boolean;
    onSelectEngine: (engineId: string) => void | Promise<void>;
    onSelectModel: (modelId: string) => void | Promise<void>;
    onSelectBackend: (backendId: string) => void | Promise<void>;
    onDownloadModel: () => void | Promise<void>;
    onLoadModel: () => void | Promise<void>;
  } = $props();

  let selectedEngine = $derived(getSelectedTranscriptionEngine(catalog, selectedEngineId));
  let selectedModel = $derived(getSelectedTranscriptionModel(catalog, selectedEngineId, selectedModelId));
  let selectedBackend = $derived(
    getSelectedTranscriptionBackend(catalog, selectedEngineId, selectedModelId, selectedBackendId),
  );
  let availableModels = $derived(selectedEngine?.models ?? []);
  let availableBackends = $derived(selectedModel?.backends ?? []);
  let selectedAvailabilityNote = $derived(
    selectedModel?.availability_note ?? selectedBackend?.availability_note ?? null,
  );

  function formatDownloadSize(bytes: number | null): string {
    if (!bytes || bytes <= 0) return "Download size varies by provider.";
    return `${(bytes / 1_000_000_000).toFixed(1)} GB download`;
  }

  function formatMemory(bytes: number | null): string | null {
    if (!bytes || bytes <= 0) return null;
    return `${(bytes / 1_000_000_000).toFixed(1)} GB RAM/VRAM`;
  }
</script>

<section class="surface-card flex flex-col gap-4">
  <div class="flex items-start justify-between gap-4 flex-wrap">
    <div class="flex-1 min-w-0">
      <h3>Transcription Model</h3>
      <p class="text-text-secondary text-sm">
        Select the active engine profile. Downloads, runtime status, and future model families use the same contract.
      </p>
    </div>

    <span class={`pill ${modelLoaded ? "pill-success" : modelDownloaded ? "pill-warning" : "pill-muted"}`}>
      {modelLoaded ? "Ready" : modelDownloaded ? "Downloaded" : "Not downloaded"}
    </span>
  </div>

  {#if catalog}
    <div class="grid grid-cols-[minmax(220px,0.9fr)_minmax(260px,1.1fr)] gap-3 max-[700px]:grid-cols-1">
      <div class="flex flex-col gap-1.5">
        <label for="transcription-engine" class="field-label">Engine</label>
        <select
          id="transcription-engine"
          value={selectedEngineId}
          onchange={(event) => onSelectEngine((event.currentTarget as HTMLSelectElement).value)}
          class="field-select"
        >
          {#each catalog.engines as engine}
            <option value={engine.id} disabled={!hasAvailableTranscriptionModel(engine)}>
              {engine.label}{hasAvailableTranscriptionModel(engine) ? "" : " (Coming soon)"}
            </option>
          {/each}
        </select>
        {#if selectedEngine}
          <p class="text-sm text-text-muted">{selectedEngine.description}</p>
        {/if}
      </div>

      <div class="flex flex-col gap-1.5">
        <label for="transcription-model" class="field-label">Model</label>
        <select
          id="transcription-model"
          value={selectedModelId}
          onchange={(event) => onSelectModel((event.currentTarget as HTMLSelectElement).value)}
          class="field-select"
          disabled={availableModels.length === 0}
        >
          {#each availableModels as model}
            <option value={model.id} disabled={!isTranscriptionModelAvailable(model)}>
              {model.label}{isTranscriptionModelAvailable(model) ? "" : " (Coming soon)"}
            </option>
          {/each}
        </select>
        {#if selectedModel}
          <div class="flex items-center gap-2 flex-wrap text-sm text-text-muted">
            <span>{selectedModel.description}</span>
            <span class="pill pill-muted">{formatDownloadSize(selectedModel.download_size_bytes)}</span>
            {#if formatMemory(selectedModel.recommended_memory_bytes)}
              <span class="pill pill-muted">{formatMemory(selectedModel.recommended_memory_bytes)}</span>
            {/if}
            {#if selectedModel.supported_languages.length > 0}
              <span class="pill pill-muted">Languages: {selectedModel.supported_languages.join(", ")}</span>
            {/if}
            <span class="pill pill-muted">
              {selectedModel.capabilities.supports_streaming ? "Streaming" : "Batch"}
            </span>
          </div>
        {/if}
      </div>
    </div>

    {#if availableBackends.length > 1}
      <div class="flex flex-col gap-1.5">
        <label for="transcription-backend" class="field-label">Runtime backend</label>
        <select
          id="transcription-backend"
          value={selectedBackendId}
          onchange={(event) => onSelectBackend((event.currentTarget as HTMLSelectElement).value)}
          class="field-select"
        >
          {#each availableBackends as backend}
            <option value={backend.id} disabled={!isTranscriptionBackendAvailable(backend)}>
              {backend.label}{isTranscriptionBackendAvailable(backend) ? "" : " (Coming soon)"}
            </option>
          {/each}
        </select>
        {#if selectedBackend}
          <p class="text-sm text-text-muted">{selectedBackend.description}</p>
        {/if}
      </div>
    {:else if selectedBackend}
      <div class="rounded-default bg-surface-3 px-4 py-3 outline-1 outline-ghost-border">
        <strong class="block text-text-primary">{selectedBackend.label} runtime</strong>
        <p class="text-text-muted text-sm">{selectedBackend.description}</p>
      </div>
    {/if}
  {/if}

  {#if selectedAvailabilityNote}
    <div class="rounded-default bg-surface-3 px-4 py-3 outline-1 outline-ghost-border">
      <strong class="block text-text-primary">Provider roadmap</strong>
      <p class="text-text-muted text-sm">{selectedAvailabilityNote}</p>
    </div>
  {/if}

  <div class="flex items-center justify-between gap-4 flex-wrap">
    <div class="min-w-0">
      <strong class="block text-text-primary">
        {!modelDownloaded
          ? `Download ${selectedModel?.label ?? "the selected model"}`
          : !modelLoaded
            ? `Load ${selectedModel?.label ?? "the selected model"}`
            : `${selectedModel?.label ?? "Selected model"} is ready`}
      </strong>
      <p class="text-text-muted text-sm">
        {!modelDownloaded
          ? "The model is stored locally and reused across sessions."
          : !modelLoaded
            ? "The first load warms up the tokenizer, weights, and Metal kernels."
            : "You can switch profiles at any time. The runtime status follows the selected DTO."}
      </p>
    </div>

    {#if !modelDownloaded}
      <button onclick={onDownloadModel} class="btn btn-primary" disabled={isDownloading}>
        {#if isDownloading}
          <Spinner />
          Downloading...
        {:else}
          Download model
        {/if}
      </button>
    {:else if !modelLoaded}
      <button onclick={onLoadModel} class="btn btn-primary" disabled={isLoadingModel}>
        {#if isLoadingModel}
          <Spinner />
          Loading...
        {:else}
          Load model
        {/if}
      </button>
    {:else}
      <button class="btn" disabled>Model ready</button>
    {/if}
  </div>

  {#if isDownloading || isLoadingModel}
    <div class="rounded-default bg-surface-3 px-4 py-3 outline-1 outline-ghost-border">
      <strong>{isDownloading ? (downloadFile || "Downloading model files...") : "Preparing engine..."}</strong>
      <p class="text-text-muted text-sm">
        {isDownloading ? "Keep the app open while files download." : "Usually takes a few seconds on first load."}
      </p>
    </div>
  {/if}
</section>
