<script lang="ts">
  import { RefreshCw } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import SettingsField from "../../../components/ui/SettingsField.svelte";
  import type { AudioDeviceInfo } from "../../../types";

  let {
    audioDevices,
    selectedDevice,
    vadEnabled,
    fillerRemoval,
    stutterCollapse,
    dictionaryCorrection,
    onDeviceChange,
    onRefreshDevices,
    onVadEnabledChange,
    onFillerRemovalChange,
    onStutterCollapseChange,
    onDictionaryCorrectionChange,
  }: {
    audioDevices: AudioDeviceInfo[];
    selectedDevice: string;
    vadEnabled: boolean;
    fillerRemoval: boolean;
    stutterCollapse: boolean;
    dictionaryCorrection: boolean;
    onDeviceChange: (event: Event) => void | Promise<void>;
    onRefreshDevices: () => void | Promise<void>;
    onVadEnabledChange: (event: Event) => void | Promise<void>;
    onFillerRemovalChange: (event: Event) => void | Promise<void>;
    onStutterCollapseChange: (event: Event) => void | Promise<void>;
    onDictionaryCorrectionChange: (event: Event) => void | Promise<void>;
  } = $props();
</script>

<section class="surface-card flex flex-col gap-3.5">
  <h3>{$t("settings_audio.title")}</h3>
  <p class="text-text-secondary text-sm">{$t("settings_audio.description")}</p>

  <div class="flex flex-col gap-1.5">
    <label for="input-device" class="field-label">{$t("settings_audio.input_device")}</label>
    <div class="flex gap-1.5 items-center">
      <select id="input-device" value={selectedDevice} onchange={onDeviceChange} class="field-select">
        {#each audioDevices as device}
          <option value={device.name}>
            {device.name}{device.is_default ? ` ${$t("settings_audio.device_default_suffix")}` : ""}
          </option>
        {/each}
      </select>
      <button onclick={onRefreshDevices} class="btn btn-icon" aria-label={$t("settings_audio.refresh_devices")}>
        <RefreshCw size={16} />
      </button>
    </div>
  </div>

  <SettingsField
    label={$t("settings_audio.vad_enabled")}
    description={$t("settings_audio.vad_enabled_desc")}
  >
    {#snippet control()}
      <input type="checkbox" checked={vadEnabled} onchange={onVadEnabledChange} class="switch" aria-label={$t("settings_audio.vad_enabled")} />
    {/snippet}
  </SettingsField>

  <SettingsField
    label={$t("settings_audio.filler_removal")}
    description={$t("settings_audio.filler_removal_desc")}
  >
    {#snippet control()}
      <input type="checkbox" checked={fillerRemoval} onchange={onFillerRemovalChange} class="switch" aria-label={$t("settings_audio.filler_removal")} />
    {/snippet}
  </SettingsField>

  <SettingsField
    label={$t("settings_audio.stutter_collapse")}
    description={$t("settings_audio.stutter_collapse_desc")}
  >
    {#snippet control()}
      <input type="checkbox" checked={stutterCollapse} onchange={onStutterCollapseChange} class="switch" aria-label={$t("settings_audio.stutter_collapse")} />
    {/snippet}
  </SettingsField>

  <SettingsField
    label={$t("settings_audio.dictionary_correction")}
    description={$t("settings_audio.dictionary_correction_desc")}
  >
    {#snippet control()}
      <input type="checkbox" checked={dictionaryCorrection} onchange={onDictionaryCorrectionChange} class="switch" aria-label={$t("settings_audio.dictionary_correction")} />
    {/snippet}
  </SettingsField>
</section>
