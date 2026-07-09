<script lang="ts">
  import { t } from "svelte-i18n";
  import SettingsField from "../../../components/ui/SettingsField.svelte";

  let {
    enabled,
    volume,
    onEnabledChange,
    onVolumeChange,
  }: {
    enabled: boolean;
    volume: number;
    onEnabledChange: (event: Event) => void;
    onVolumeChange: (event: Event) => void;
  } = $props();
</script>

<section class="settings-group">
  <h3>{$t("settings_feedback_sounds.title")}</h3>
  <div class="settings-rows">
    <SettingsField
      label={$t("settings_feedback_sounds.enabled")}
      description={$t("settings_feedback_sounds.enabled_desc")}
    >
      {#snippet control()}
        <input
          type="checkbox"
          checked={enabled}
          onchange={onEnabledChange}
          class="switch"
          aria-label={$t("settings_feedback_sounds.enabled")}
        />
      {/snippet}
    </SettingsField>

    {#if enabled}
      <SettingsField
        label={$t("settings_feedback_sounds.volume")}
        description={$t("settings_feedback_sounds.volume_desc")}
        htmlFor="feedback-sounds-volume"
      >
        {#snippet control()}
          <div class="flex items-center gap-3 max-w-xs">
            <input
              id="feedback-sounds-volume"
              type="range"
              min="0"
              max="100"
              value={volume}
              onchange={onVolumeChange}
              class="w-full"
            />
            <span class="text-sm text-text-muted tabular-nums w-10 text-right">{volume}%</span>
          </div>
        {/snippet}
      </SettingsField>
    {/if}
  </div>
</section>
