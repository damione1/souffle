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

<section class="settings-group">
  <h3>{$t("settings_model.title")}</h3>
  <div class="settings-rows">
    <div class="flex items-center justify-between gap-4">
      <div class="flex min-w-0 flex-1 flex-col gap-0.5">
        <span class="setting-label">{$t("settings_model.title")}</span>
        <span class="setting-desc">{$t("settings_model.description")}</span>
      </div>
      <StatusChip
        phase={runtimePhase}
        operationState={operationState}
        {downloadedBytes}
        {downloadTotalBytes}
      />
    </div>

    <div>
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
    </div>

    {#if operationState === "downloading"}
      <div>
        <ProgressBar
          value={downloadedBytes}
          max={downloadTotalBytes && downloadTotalBytes > 0 ? downloadTotalBytes : 100}
          label={downloadFile || $t("settings_model.downloading")}
        />
      </div>
    {/if}
  </div>
</section>
