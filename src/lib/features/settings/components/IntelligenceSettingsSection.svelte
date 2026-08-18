<script lang="ts">
  import { t } from "svelte-i18n";
  import ProgressBar from "../../../components/ui/ProgressBar.svelte";
  import SettingsField from "../../../components/ui/SettingsField.svelte";
  import StatusBanner from "../../../components/ui/StatusBanner.svelte";
  import type { SummaryModelDescriptor } from "../../../types";

  let {
    ollamaUrl,
    ollamaAvailable,
    appleIntelligenceAvailable,
    appleIntelligenceUnavailableReason = null,
    ollamaModels,
    summaryModels,
    selectedOllamaModel,
    recommendedOllamaModel,
    ollamaPulling,
    ollamaPullStatus,
    ollamaPullDownloaded,
    ollamaPullTotal,
    ollamaPullError,
    onOllamaUrlChange,
    onOllamaModelChange,
    onRetrySummaryProviders,
    onDownloadRecommendedOllamaModel,
  }: {
    ollamaUrl: string;
    ollamaAvailable: boolean;
    appleIntelligenceAvailable: boolean;
    appleIntelligenceUnavailableReason?: string | null;
    ollamaModels: SummaryModelDescriptor[];
    summaryModels: SummaryModelDescriptor[];
    selectedOllamaModel: string;
    recommendedOllamaModel: string;
    ollamaPulling: boolean;
    ollamaPullStatus: string;
    ollamaPullDownloaded: number;
    ollamaPullTotal: number | null;
    ollamaPullError: string;
    onOllamaUrlChange: (event: Event) => void;
    onOllamaModelChange: (event: Event) => void;
    onRetrySummaryProviders: () => void | Promise<void>;
    onDownloadRecommendedOllamaModel: () => void | Promise<void>;
  } = $props();

  const KNOWN_REASON_KEYS: Record<string, string> = {
    device_not_eligible: "settings_intelligence.ai_reason_device_not_eligible",
    apple_intelligence_not_enabled: "settings_intelligence.ai_reason_apple_intelligence_not_enabled",
    model_not_ready: "settings_intelligence.ai_reason_model_not_ready",
    macos_too_old: "settings_intelligence.ai_reason_macos_too_old",
    stub: "settings_intelligence.ai_reason_stub",
    unsupported_platform: "settings_intelligence.ai_reason_unsupported_platform",
  };

  let appleIntelligenceHintKey = $derived(
    !appleIntelligenceAvailable && appleIntelligenceUnavailableReason
      ? (KNOWN_REASON_KEYS[appleIntelligenceUnavailableReason] ?? "settings_intelligence.ai_reason_unknown")
      : null,
  );
</script>

<section class="settings-group">
  <h3>{$t("settings_intelligence.title")}</h3>
  <div class="settings-rows">
  <SettingsField
    label={$t("settings_intelligence.apple_intelligence")}
    description={$t("settings_intelligence.apple_intelligence_desc")}
  >
    {#snippet control()}
      <div class="flex gap-2 items-center">
        <span class="status-dot" class:is-online={appleIntelligenceAvailable}></span>
        <span class="text-sm text-text-muted">
          {appleIntelligenceAvailable
            ? $t("settings_intelligence.apple_available")
            : $t("settings_intelligence.apple_not_available")}
        </span>
      </div>
    {/snippet}
  </SettingsField>

  {#if appleIntelligenceHintKey}
    <StatusBanner
      message={appleIntelligenceHintKey === "settings_intelligence.ai_reason_unknown"
        ? $t(appleIntelligenceHintKey, { values: { code: appleIntelligenceUnavailableReason } })
        : $t(appleIntelligenceHintKey)}
    />
  {/if}

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
        <button onclick={onRetrySummaryProviders} class="btn" disabled={ollamaPulling}>{$t("settings_intelligence.retry")}</button>
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
  {:else if ollamaAvailable}
    {#if ollamaModels.length > 0}
      <StatusBanner message={$t("settings_intelligence.no_compatible_model")} />
    {/if}
    {#if ollamaPullError}
      <StatusBanner variant="danger" message={$t("settings_intelligence.pull_failed", { values: { error: ollamaPullError } })} />
    {/if}
    {#if ollamaPulling}
      <ProgressBar
        value={ollamaPullDownloaded}
        max={ollamaPullTotal && ollamaPullTotal > 0 ? ollamaPullTotal : 100}
        label={ollamaPullStatus || $t("settings_intelligence.downloading_model", { values: { model: recommendedOllamaModel } })}
      />
    {:else}
      <SettingsField
        label={$t("settings_intelligence.recommended_model")}
        description={$t("settings_intelligence.download_recommended_desc")}
      >
        {#snippet control()}
          <button onclick={onDownloadRecommendedOllamaModel} class="btn btn-primary">
            {$t("settings_intelligence.download_recommended", { values: { model: recommendedOllamaModel } })}
          </button>
        {/snippet}
      </SettingsField>
    {/if}
  {/if}
  </div>
</section>
