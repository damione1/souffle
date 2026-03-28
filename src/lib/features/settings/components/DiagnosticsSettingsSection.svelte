<script lang="ts">
  import { t } from "svelte-i18n";

  let {
    debugTranscription,
    onDebugTranscriptionChange,
  }: {
    debugTranscription: boolean;
    onDebugTranscriptionChange: (event: Event) => void;
  } = $props();
</script>

<section class="surface-card flex flex-col gap-3.5">
  <h3>{$t("settings_diagnostics.title")}</h3>

  <div class="flex items-center justify-between gap-4">
    <div>
      <span class="block text-[0.9375rem] font-medium text-text-primary">{$t("settings_diagnostics.debug_logs")}</span>
      <span class="text-sm text-text-muted">{$t("settings_diagnostics.debug_logs_desc")}</span>
    </div>
    <input
      type="checkbox"
      checked={debugTranscription}
      onchange={onDebugTranscriptionChange}
      class="switch"
      aria-label={$t("settings_diagnostics.debug_logs_aria")}
    />
  </div>
</section>
