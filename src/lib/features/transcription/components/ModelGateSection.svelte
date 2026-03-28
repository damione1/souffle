<script lang="ts">
  import { t } from "svelte-i18n";
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
    runtimePhaseAvailabilityLabelKey,
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
    if (!bytes || bytes <= 0) return $t("model_gate.download_size_varies");
    return $t("model_gate.gb_download", { values: { size: (bytes / 1_000_000_000).toFixed(1) } });
  }

  function formatMemory(bytes: number | null): string | null {
    if (!bytes || bytes <= 0) return null;
    return $t("model_gate.gb_memory", { values: { size: (bytes / 1_000_000_000).toFixed(1) } });
  }

  let downloadLabel = $derived.by(() => {
    if (phase !== "downloading") return "";
    if (downloadTotalFiles > 0) {
      return $t("model_gate.files_progress", { values: { completed: downloadCompletedFiles, total: downloadTotalFiles } });
    }
    return $t("model_gate.preparing_download");
  });
</script>

<section class="surface-card flex flex-col gap-4">
  <div class="flex items-start justify-between gap-4 flex-wrap">
    <div class="flex-1 min-w-0">
      <h3>{$t("model_gate.title")}</h3>
      <p class="text-text-secondary text-sm">
        {$t("model_gate.description")}
      </p>
    </div>

    <span class={`pill ${runtimePhasePillClass(runtimePhase)}`}>
      {$t(runtimePhaseAvailabilityLabelKey(runtimePhase))}
    </span>
  </div>

  {#if catalog}
    <div class="grid grid-cols-[minmax(220px,0.9fr)_minmax(260px,1.1fr)] gap-3 max-[700px]:grid-cols-1">
      <div class="flex flex-col gap-1.5">
        <label for="transcription-engine" class="field-label">{$t("model_gate.engine")}</label>
        <select
          id="transcription-engine"
          value={selectedEngineId}
          onchange={(event) => onSelectEngine((event.currentTarget as HTMLSelectElement).value)}
          class="field-select"
          disabled={isBusy}
        >
          {#each catalog.engines as engine}
            <option value={engine.id} disabled={!hasAvailableTranscriptionModel(engine)}>
              {engine.label}{hasAvailableTranscriptionModel(engine) ? "" : ` ${$t("model_gate.coming_soon_suffix")}`}
            </option>
          {/each}
        </select>
        {#if selectedEngine}
          <p class="text-sm text-text-muted">{selectedEngine.description}</p>
        {/if}
      </div>

      <div class="flex flex-col gap-1.5">
        <label for="transcription-model" class="field-label">{$t("model_gate.model")}</label>
        <select
          id="transcription-model"
          value={selectedModelId}
          onchange={(event) => onSelectModel((event.currentTarget as HTMLSelectElement).value)}
          class="field-select"
          disabled={availableModels.length === 0 || isBusy}
        >
          {#each availableModels as model}
            <option value={model.id} disabled={!isTranscriptionModelAvailable(model)}>
              {model.label}{isTranscriptionModelAvailable(model) ? "" : ` ${$t("model_gate.coming_soon_suffix")}`}
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
              <span class="pill pill-muted">{$t("model_gate.languages_label")} {selectedModel.supported_languages.join(", ")}</span>
            {/if}
            <span class="pill pill-muted">
              {selectedModel.capabilities.supports_streaming ? $t("model_gate.streaming") : $t("model_gate.batch")}
            </span>
          </div>
        {/if}
      </div>
    </div>

    {#if availableBackends.length > 1}
      <div class="flex flex-col gap-1.5">
        <label for="transcription-backend" class="field-label">{$t("model_gate.runtime")}</label>
        <select
          id="transcription-backend"
          value={selectedBackendId}
          onchange={(event) => onSelectBackend((event.currentTarget as HTMLSelectElement).value)}
          class="field-select"
          disabled={isBusy}
        >
          {#each availableBackends as backend}
            <option value={backend.id} disabled={!isTranscriptionBackendAvailable(backend)}>
              {backend.label}{isTranscriptionBackendAvailable(backend) ? "" : ` ${$t("model_gate.coming_soon_suffix")}`}
            </option>
          {/each}
        </select>
        {#if selectedBackend}
          <p class="text-sm text-text-muted">{selectedBackend.description}</p>
        {/if}
      </div>
    {:else if selectedBackend}
      <div class="rounded-default bg-surface-3 px-4 py-3 outline-1 outline-ghost-border">
        <strong class="block text-text-primary">{selectedBackend.label} {$t("model_gate.runtime_suffix")}</strong>
        <p class="text-text-muted text-sm">{selectedBackend.description}</p>
      </div>
    {/if}
  {/if}

  {#if selectedAvailabilityNote}
    <div class="rounded-default bg-surface-3 px-4 py-3 outline-1 outline-ghost-border">
      <strong class="block text-text-primary">{$t("model_gate.availability")}</strong>
      <p class="text-text-muted text-sm">{selectedAvailabilityNote}</p>
    </div>
  {/if}

  {#if phase === "needs_download" || phase === "downloading"}
    <div class="flex items-center justify-between gap-4 flex-wrap">
      <div class="min-w-0">
        <p class="text-sm font-medium text-text-primary">{$t("model_gate.download_title", { values: { model: selectedModel?.label ?? "" } })}</p>
        <p class="text-text-muted text-sm">{$t("model_gate.download_stored_info")}</p>
      </div>
      <button onclick={onDownloadModel} class="btn btn-primary" disabled={phase === "downloading"}>
        {#if phase === "downloading"}<Spinner /> {$t("model_gate.downloading")}{:else}{$t("model_gate.download_model")}{/if}
      </button>
    </div>
  {:else if phase === "needs_load" || phase === "loading"}
    <div class="flex items-center justify-between gap-4 flex-wrap">
      <div class="min-w-0">
        <p class="text-sm font-medium text-text-primary">{$t("model_gate.load_title", { values: { model: selectedModel?.label ?? "" } })}</p>
        <p class="text-text-muted text-sm">{$t("model_gate.load_info")}</p>
      </div>
      <button onclick={onLoadModel} class="btn btn-primary" disabled={phase === "loading"}>
        {#if phase === "loading"}<Spinner /> {$t("model_gate.loading")}{:else}{$t("model_gate.load_model")}{/if}
      </button>
    </div>
  {/if}

  {#if phase === "downloading"}
    <div class="rounded-default bg-surface-3 px-4 py-3 outline-1 outline-ghost-border">
      <strong>{downloadFile || $t("model_gate.downloading_files")}</strong>
      <p class="text-text-muted text-sm">{$t("model_gate.keep_app_open")}</p>
      <div class="mt-3">
        <ProgressBar value={downloadCompletedFiles} max={downloadTotalFiles || 1} label={downloadLabel} />
      </div>
    </div>
  {:else if phase === "loading"}
    <div class="rounded-default bg-surface-3 px-4 py-3 outline-1 outline-ghost-border">
      <strong>{$t("model_gate.loading_model")}</strong>
      <p class="text-text-muted text-sm">{$t("model_gate.usually_few_seconds")}</p>
    </div>
  {/if}
</section>
