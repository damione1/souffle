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

<section class="surface-card flex flex-col gap-3.5">
  <h3>{$t("settings_about.title")}</h3>

  <div class="flex flex-col gap-2">
    <div class="flex justify-between gap-4">
      <span class="text-text-muted text-sm">{$t("settings_about.version")}</span>
      <span class="text-sm">v0.1.0</span>
    </div>
    <div class="flex justify-between gap-4">
      <span class="text-text-muted text-sm">{$t("settings_about.transcription")}</span>
      <span class="text-sm text-right">{selectedTranscriptionLabel}</span>
    </div>
    <div class="flex justify-between gap-4">
      <span class="text-text-muted text-sm">{$t("settings_about.summaries")}</span>
      <span class="text-sm text-right">{selectedOllamaModelLabel || $t("settings_about.not_configured")}</span>
    </div>
    <div class="flex justify-between gap-4">
      <span class="text-text-muted text-sm">{$t("settings_about.privacy")}</span>
      <span class="text-sm">{$t("settings_about.privacy_value")}</span>
    </div>
  </div>
</section>
