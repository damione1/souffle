<script lang="ts">
  import { t } from "svelte-i18n";

  let {
    selectedTranscriptionLabel,
    selectedOllamaModelLabel,
  }: {
    selectedTranscriptionLabel: string;
    selectedOllamaModelLabel: string;
  } = $props();
</script>

<section class="settings-group">
  <h3>{$t("settings_about.title")}</h3>
  <div class="settings-rows">
    <div class="flex justify-between gap-4">
      <span class="setting-desc">{$t("settings_about.version")}</span>
      <span class="text-[13px] text-text-secondary">v0.1.0</span>
    </div>
    <div class="flex justify-between gap-4">
      <span class="setting-desc">{$t("settings_about.transcription")}</span>
      <span class="text-[13px] text-right text-text-secondary">{selectedTranscriptionLabel}</span>
    </div>
    <div class="flex justify-between gap-4">
      <span class="setting-desc">{$t("settings_about.summaries")}</span>
      <span class="text-[13px] text-right text-text-secondary">{selectedOllamaModelLabel || $t("settings_about.not_configured")}</span>
    </div>
    <div class="flex justify-between gap-4">
      <span class="setting-desc">{$t("settings_about.privacy")}</span>
      <span class="text-[13px] text-text-secondary">{$t("settings_about.privacy_value")}</span>
    </div>
  </div>
</section>
