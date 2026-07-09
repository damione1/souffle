<script lang="ts">
  import { t } from "svelte-i18n";
  import ProgressBar from "../../../components/ui/ProgressBar.svelte";
  import SettingsField from "../../../components/ui/SettingsField.svelte";
  import StatusChip from "../../../components/ui/StatusChip.svelte";
  import { listAvailableModelOptions } from "../../transcription/catalog";
  import type { TranscriptionModelOperationState } from "../../transcription/state";
  import type { TranscriptionCatalog, TranscriptionRuntimePhase } from "../../../types";

  const unloadTimeoutOptions = [0, 5, 15, 60] as const;
  const unloadTimeoutKeys: Record<(typeof unloadTimeoutOptions)[number], string> = {
    0: "settings_model.unload_timeout_never",
    5: "settings_model.unload_timeout_5min",
    15: "settings_model.unload_timeout_15min",
    60: "settings_model.unload_timeout_1hour",
  };

  let {
    catalog,
    selectedEngineId,
    selectedModelId,
    runtimePhase,
    operationState,
    downloadedBytes,
    downloadTotalBytes,
    downloadFile,
    unloadTimeoutMinutes,
    onSelectModel,
    onUnloadTimeoutChange,
  }: {
    catalog: TranscriptionCatalog | null;
    selectedEngineId: string;
    selectedModelId: string;
    runtimePhase: TranscriptionRuntimePhase;
    operationState: TranscriptionModelOperationState;
    downloadedBytes: number;
    downloadTotalBytes: number | null;
    downloadFile: string;
    unloadTimeoutMinutes: number;
    onSelectModel: (key: string) => void | Promise<void>;
    onUnloadTimeoutChange: (event: Event) => void;
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

    <SettingsField
      label={$t("settings_model.unload_timeout_label")}
      description={$t("settings_model.unload_timeout_desc")}
      htmlFor="model-unload-timeout"
    >
      {#snippet control()}
        <select
          id="model-unload-timeout"
          value={unloadTimeoutMinutes}
          onchange={onUnloadTimeoutChange}
          class="field-select max-w-48"
        >
          {#each unloadTimeoutOptions as minutes}
            <option value={minutes}>{$t(unloadTimeoutKeys[minutes])}</option>
          {/each}
        </select>
      {/snippet}
    </SettingsField>
  </div>
</section>
