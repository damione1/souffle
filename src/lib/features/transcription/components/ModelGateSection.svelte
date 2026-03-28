<script lang="ts">
  import ProgressBar from "../../../components/ui/ProgressBar.svelte";
  import Spinner from "../../../components/ui/Spinner.svelte";
  import type { TranscriptionCatalog, TranscriptionRuntimePhase } from "../../../types";
  import {
    hasAvailableTranscriptionModel,
    isTranscriptionBackendAvailable,
    isTranscriptionModelAvailable,
    getSelectedTranscriptionBackend,
    getSelectedTranscriptionEngine,
    getSelectedTranscriptionModel,
  } from "../catalog";
  import {
    runtimePhaseAvailabilityLabel,
    runtimePhasePillClass,
    type TranscriptionModelOperationState,
  } from "../state";

  type ModelGatePhase = "downloading" | "needs_download" | "loading" | "needs_load" | "ready";

  let {
    catalog,
    selectedEngineId,
    selectedModelId,
    selectedBackendId,
    runtimePhase,
    modelOperationState,
    downloadFile,
    downloadCompletedFiles,
    downloadTotalFiles,
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
    runtimePhase: TranscriptionRuntimePhase;
    modelOperationState: TranscriptionModelOperationState;
    downloadFile: string;
    downloadCompletedFiles: number;
    downloadTotalFiles: number;
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

  let phase = $derived.by((): ModelGatePhase => {
    if (modelOperationState === "downloading") return "downloading";
    if (runtimePhase === "download_required") return "needs_download";
    if (modelOperationState === "loading") return "loading";
    if (runtimePhase !== "ready") return "needs_load";
    return "ready";
  });

  let isBusy = $derived(phase === "downloading" || phase === "loading");

  function formatDownloadSize(bytes: number | null): string {
    if (!bytes || bytes <= 0) return "Download size varies by provider.";
    return `${(bytes / 1_000_000_000).toFixed(1)} GB download`;
  }

  function formatMemory(bytes: number | null): string | null {
    if (!bytes || bytes <= 0) return null;
    return `${(bytes / 1_000_000_000).toFixed(1)} GB RAM/VRAM`;
  }

  let downloadLabel = $derived.by(() => {
    if (phase !== "downloading") return "";
    if (downloadTotalFiles > 0) {
      return `${downloadCompletedFiles}/${downloadTotalFiles} files`;
    }
    return "Preparing download";
  });
</script>

<section class="surface-card flex flex-col gap-4">
  <div class="flex items-start justify-between gap-4 flex-wrap">
    <div class="flex-1 min-w-0">
      <h3>Transcription Model</h3>
      <p class="text-text-secondary text-sm">
        Choose the speech-to-text model Souffle uses for dictation and meetings.
      </p>
    </div>

    <span class={`pill ${runtimePhasePillClass(runtimePhase)}`}>
      {runtimePhaseAvailabilityLabel(runtimePhase)}
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
          disabled={isBusy}
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
          disabled={availableModels.length === 0 || isBusy}
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
        <label for="transcription-backend" class="field-label">Runtime</label>
        <select
          id="transcription-backend"
          value={selectedBackendId}
          onchange={(event) => onSelectBackend((event.currentTarget as HTMLSelectElement).value)}
          class="field-select"
          disabled={isBusy}
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
      <strong class="block text-text-primary">Availability</strong>
      <p class="text-text-muted text-sm">{selectedAvailabilityNote}</p>
    </div>
  {/if}

  {#if phase === "needs_download" || phase === "downloading"}
    <div class="flex items-center justify-between gap-4 flex-wrap">
      <div class="min-w-0">
        <p class="text-sm font-medium text-text-primary">Download {selectedModel?.label ?? "the selected model"}</p>
        <p class="text-text-muted text-sm">The model is stored on your Mac and reused across sessions.</p>
      </div>
      <button onclick={onDownloadModel} class="btn btn-primary" disabled={phase === "downloading"}>
        {#if phase === "downloading"}<Spinner /> Downloading...{:else}Download model{/if}
      </button>
    </div>
  {:else if phase === "needs_load" || phase === "loading"}
    <div class="flex items-center justify-between gap-4 flex-wrap">
      <div class="min-w-0">
        <p class="text-sm font-medium text-text-primary">Load {selectedModel?.label ?? "the selected model"}</p>
        <p class="text-text-muted text-sm">First load may take a few seconds while the model is prepared.</p>
      </div>
      <button onclick={onLoadModel} class="btn btn-primary" disabled={phase === "loading"}>
        {#if phase === "loading"}<Spinner /> Loading...{:else}Load model{/if}
      </button>
    </div>
  {/if}

  {#if phase === "downloading"}
    <div class="rounded-default bg-surface-3 px-4 py-3 outline-1 outline-ghost-border">
      <strong>{downloadFile || "Downloading model files..."}</strong>
      <p class="text-text-muted text-sm">Keep the app open while files download.</p>
      <div class="mt-3">
        <ProgressBar value={downloadCompletedFiles} max={downloadTotalFiles || 1} label={downloadLabel} />
      </div>
    </div>
  {:else if phase === "loading"}
    <div class="rounded-default bg-surface-3 px-4 py-3 outline-1 outline-ghost-border">
      <strong>Loading model...</strong>
      <p class="text-text-muted text-sm">Usually takes a few seconds.</p>
    </div>
  {/if}
</section>
