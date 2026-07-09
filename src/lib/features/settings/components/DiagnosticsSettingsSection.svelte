<script lang="ts">
  import { t } from "svelte-i18n";
  import SettingsField from "../../../components/ui/SettingsField.svelte";

  let {
    debugTranscription,
    onDebugTranscriptionChange,
  }: {
    debugTranscription: boolean;
    onDebugTranscriptionChange: (event: Event) => void;
  } = $props();
</script>

<section class="settings-group">
  <h3>{$t("settings_diagnostics.title")}</h3>
  <div class="settings-rows">
  <SettingsField
    label={$t("settings_diagnostics.debug_logs")}
    description={$t("settings_diagnostics.debug_logs_desc")}
  >
    {#snippet control()}
      <input
        type="checkbox"
        checked={debugTranscription}
        onchange={onDebugTranscriptionChange}
        class="switch"
        aria-label={$t("settings_diagnostics.debug_logs_aria")}
      />
    {/snippet}
  </SettingsField>
  </div>
</section>
