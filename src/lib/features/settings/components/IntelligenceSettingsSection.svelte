<script lang="ts">
  import { t } from "svelte-i18n";
  import SettingsField from "../../../components/ui/SettingsField.svelte";
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

<section class="settings-group">
  <h3>{$t("settings_intelligence.title")}</h3>
  <div class="settings-rows">
  <SettingsField
    label={$t("settings_intelligence.ollama_url")}
    description={$t("settings_intelligence.ollama_url_desc")}
    htmlFor="ollama-url"
  >
    {#snippet control()}
      <input id="ollama-url" type="text" value={ollamaUrl} onchange={onOllamaUrlChange} class="field-input max-w-64" />
    {/snippet}
  </SettingsField>

  <SettingsField
    label={$t("settings_intelligence.connection_status")}
    description={$t("settings_intelligence.models_found", { values: { count: ollamaModels.length } })}
  >
    {#snippet control()}
      <div class="flex gap-2 items-center">
        <span class="status-dot" class:is-online={ollamaAvailable}></span>
        <span class="text-sm text-text-muted">{ollamaAvailable ? $t("settings_intelligence.connected") : $t("settings_intelligence.not_available")}</span>
        <button onclick={onRetryOllama} class="btn">{$t("settings_intelligence.retry")}</button>
      </div>
    {/snippet}
  </SettingsField>

  {#if ollamaAvailable && summaryModels.length > 0}
    <div class="flex items-center justify-between gap-4">
      <label for="summary-model" class="setting-label shrink-0">{$t("settings_intelligence.summary_model")}</label>
      <select
        id="summary-model"
        value={selectedOllamaModel || summaryModels[0].id}
        onchange={onOllamaModelChange}
        class="field-select max-w-64"
      >
        {#each summaryModels as model}
          <option value={model.id}>{model.label}</option>
        {/each}
      </select>
    </div>
  {:else if ollamaAvailable && ollamaModels.length > 0}
    <StatusBanner message={$t("settings_intelligence.no_compatible_model")} />
  {/if}
  </div>
</section>
