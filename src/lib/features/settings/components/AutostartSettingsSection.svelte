<script lang="ts">
  import { t } from "svelte-i18n";
  import SettingsField from "../../../components/ui/SettingsField.svelte";

  let {
    autostartEnabled,
    onAutostartChange,
  }: {
    autostartEnabled: boolean;
    onAutostartChange: (event: Event) => void;
  } = $props();
</script>

<section class="settings-group">
  <h3>{$t("settings_startup.title")}</h3>
  <div class="settings-rows">
    <SettingsField
      label={$t("settings_startup.autostart")}
      description={$t("settings_startup.autostart_desc")}
    >
      {#snippet control()}
        <input
          type="checkbox"
          checked={autostartEnabled}
          onchange={onAutostartChange}
          class="switch"
          aria-label={$t("settings_startup.autostart")}
        />
      {/snippet}
    </SettingsField>
  </div>
</section>
