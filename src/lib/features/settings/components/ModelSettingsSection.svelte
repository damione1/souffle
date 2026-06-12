<script lang="ts">
  import { t } from "svelte-i18n";
  import ProgressBar from "../../../components/ui/ProgressBar.svelte";
  import StatusChip from "../../../components/ui/StatusChip.svelte";
  import { listAvailableModelOptions } from "../../transcription/catalog";
  import type { TranscriptionModelOperationState } from "../../transcription/state";
  import type { TranscriptionCatalog, TranscriptionRuntimePhase } from "../../../types";

  let {
    catalog,
    selectedEngineId,
    selectedModelId,
    runtimePhase,
    operationState,
    downloadedBytes,
    downloadTotalBytes,
    downloadFile,
    onSelectModel,
  }: {
    catalog: TranscriptionCatalog | null;
    selectedEngineId: string;
    selectedModelId: string;
    runtimePhase: TranscriptionRuntimePhase;
    operationState: TranscriptionModelOperationState;
    downloadedBytes: number;
    downloadTotalBytes: number | null;
    downloadFile: string;
    onSelectModel: (key: string) => void | Promise<void>;
  } = $props();

  const options = $derived(listAvailableModelOptions(catalog));
  const selectedKey = $derived(`${selectedEngineId}:${selectedModelId}`);
  const busy = $derived(operationState !== "idle");
</script>

<section class="surface-card flex flex-col gap-3.5">
  <div class="flex items-center justify-between gap-2">
    <h3>{$t("settings_model.title")}</h3>
    <StatusChip
      phase={runtimePhase}
      operationState={operationState}
      {downloadedBytes}
      {downloadTotalBytes}
    />
  </div>
  <p class="text-text-secondary text-sm">{$t("settings_model.description")}</p>

  <select
    value={selectedKey}
    disabled={busy}
    onchange={(event) => void onSelectModel((event.currentTarget as HTMLSelectElement).value)}
    class="field-select"
    aria-label={$t("settings_model.title")}
  >
    {#each options as option}
      <option value={option.key}>{option.label}</option>
    {/each}
  </select>

  {#if operationState === "downloading"}
    <ProgressBar
      value={downloadedBytes}
      max={downloadTotalBytes && downloadTotalBytes > 0 ? downloadTotalBytes : 100}
      label={downloadFile || $t("settings_model.downloading")}
    />
  {/if}
</section>
