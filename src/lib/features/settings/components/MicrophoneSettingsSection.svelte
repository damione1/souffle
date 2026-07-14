<script lang="ts">
  import { RefreshCw } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import type { AudioInputDevice } from "../../../types";

  let {
    audioDevices,
    selectedDevice,
    allowBluetoothMic,
    onDeviceChange,
    onAllowBluetoothMicChange,
    onRefreshDevices,
  }: {
    audioDevices: AudioInputDevice[];
    selectedDevice: string;
    allowBluetoothMic: boolean;
    onDeviceChange: (event: Event) => void | Promise<void>;
    onAllowBluetoothMicChange: (event: Event) => void | Promise<void>;
    onRefreshDevices: () => void | Promise<void>;
  } = $props();
</script>

<section class="settings-group">
  <h3>{$t("settings_audio.microphone_title")}</h3>
  <div class="settings-rows">
    <div class="flex items-center justify-between gap-4">
      <div class="flex min-w-0 flex-1 flex-col gap-0.5">
        <label for="input-device" class="setting-label">{$t("settings_audio.input_device")}</label>
        <span class="setting-desc">{$t("settings_audio.description")}</span>
      </div>
      <div class="flex shrink-0 gap-1.5 items-center">
        <select id="input-device" value={selectedDevice} onchange={onDeviceChange} class="field-select max-w-52">
          <option value="">{$t("settings_audio.input_device_automatic")}</option>
          {#each audioDevices as device}
            <option value={device.uid}>
              {device.name}{device.is_default ? ` ${$t("settings_audio.device_default_suffix")}` : ""}
            </option>
          {/each}
        </select>
        <button onclick={onRefreshDevices} class="btn btn-icon" aria-label={$t("settings_audio.refresh_devices")}>
          <RefreshCw size={16} />
        </button>
      </div>
    </div>

    <div class="flex items-center justify-between gap-4">
      <div class="flex min-w-0 flex-1 flex-col gap-0.5">
        <span class="setting-label">{$t("settings_audio.allow_bluetooth_mic")}</span>
        <span class="setting-desc">{$t("settings_audio.allow_bluetooth_mic_desc")}</span>
      </div>
      <input
        type="checkbox"
        checked={allowBluetoothMic}
        onchange={onAllowBluetoothMicChange}
        class="switch"
        aria-label={$t("settings_audio.allow_bluetooth_mic")}
      />
    </div>
  </div>
</section>
