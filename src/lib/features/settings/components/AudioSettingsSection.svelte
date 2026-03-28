<script lang="ts">
  import { RefreshCw } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import type { AudioDeviceInfo } from "../../../types";

  let {
    audioDevices,
    selectedDevice,
    onDeviceChange,
    onRefreshDevices,
  }: {
    audioDevices: AudioDeviceInfo[];
    selectedDevice: string;
    onDeviceChange: (event: Event) => void | Promise<void>;
    onRefreshDevices: () => void | Promise<void>;
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

  <div class="flex items-center justify-between gap-4 opacity-50">
    <div>
      <span class="block text-[0.9375rem] font-medium text-text-primary">{$t("settings_audio.noise_reduction")}</span>
      <span class="text-sm text-text-muted">{$t("settings_audio.noise_reduction_desc")}</span>
    </div>
    <div class="flex gap-2 items-center">
      <span class="pill pill-muted">{$t("settings_audio.coming_soon")}</span>
      <input type="checkbox" disabled class="switch" />
    </div>
  </div>
</section>
