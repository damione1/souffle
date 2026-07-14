<script lang="ts">
  import { ChevronDown, ChevronUp, RefreshCw } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import type { InputPriority } from "../../../types";
  import {
    buildMicrophoneList,
    transportLabelKey,
    type MicrophoneListEntry,
  } from "../microphone-list";
  import type { AudioInputDevice } from "../../../types";
  import { lastSeenAge } from "../../../utils/format";

  let {
    audioDevices,
    inputPriority,
    selectedDevice,
    pinUnavailable,
    allowBluetoothMic,
    onDeviceChange,
    onAllowBluetoothMicChange,
    onRefreshDevices,
    onMoveDevice,
    onToggleHidden,
  }: {
    audioDevices: AudioInputDevice[];
    inputPriority: InputPriority;
    selectedDevice: string;
    pinUnavailable: boolean;
    allowBluetoothMic: boolean;
    onDeviceChange: (event: Event) => void | Promise<void>;
    onAllowBluetoothMicChange: (event: Event) => void | Promise<void>;
    onRefreshDevices: () => void | Promise<void>;
    onMoveDevice: (uid: string, direction: -1 | 1) => void | Promise<void>;
    onToggleHidden: (uid: string, hidden: boolean) => void | Promise<void>;
  } = $props();

  const microphoneList = $derived(buildMicrophoneList(audioDevices, inputPriority));

  function lastSeenLabel(entry: MicrophoneListEntry): string {
    if (entry.connected || entry.lastSeen === null) return "";
    const age = lastSeenAge(entry.lastSeen);
    switch (age.kind) {
      case "just_now":
        return $t("settings_audio.last_seen_just_now");
      case "minutes":
        return $t("settings_audio.last_seen_minutes", { values: { count: age.count } });
      case "hours":
        return $t("settings_audio.last_seen_hours", { values: { count: age.count } });
      case "days":
        return $t("settings_audio.last_seen_days", { values: { count: age.count } });
    }
  }
</script>

<section class="settings-group">
  <h3>{$t("settings_audio.microphone_title")}</h3>
  <div class="settings-rows">
    <div class="flex items-center justify-between gap-4">
      <div class="flex min-w-0 flex-1 flex-col gap-0.5">
        <label for="input-device" class="setting-label">{$t("settings_audio.input_device")}</label>
        <span class="setting-desc">{$t("settings_audio.description")}</span>
      </div>
      <div class="flex shrink-0 items-center gap-1.5">
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

    {#if pinUnavailable && selectedDevice}
      <p class="rounded-lg border border-warning/30 bg-warning/10 px-3 py-2 text-sm text-warning" role="status">
        {$t("settings_audio.pin_unavailable")}
      </p>
    {/if}

    <div class="flex flex-col gap-1">
      <span class="setting-label">{$t("settings_audio.priority_title")}</span>
      <span class="setting-desc">{$t("settings_audio.priority_desc")}</span>
      <ul class="mt-2 flex flex-col gap-1.5" aria-label={$t("settings_audio.priority_title")}>
        {#each microphoneList as entry, index (entry.uid)}
          <li
            class={`flex items-center gap-2 rounded-lg border px-2.5 py-2 ${
              entry.connected
                ? "border-ghost-border bg-surface-1"
                : "border-ghost-border/60 bg-surface-1/40 opacity-70"
            }`}
          >
            <div class="flex min-w-0 flex-1 flex-col gap-0.5">
              <div class="flex flex-wrap items-center gap-1.5">
                <span class="truncate text-sm font-medium">{entry.name}</span>
                <span class="rounded-full bg-surface-2 px-2 py-0.5 text-[11px] text-text-muted">
                  {$t(transportLabelKey(entry.transport))}
                </span>
                {#if entry.isDefault}
                  <span class="text-[11px] text-text-muted">{$t("settings_audio.device_default_suffix")}</span>
                {/if}
              </div>
              {#if !entry.connected && entry.lastSeen !== null}
                <span class="text-xs text-text-muted">{lastSeenLabel(entry)}</span>
              {/if}
            </div>
            <div class="flex shrink-0 items-center gap-1">
              <button
                type="button"
                class="btn btn-icon disabled:opacity-30"
                disabled={index === 0}
                aria-label={$t("settings_audio.move_up")}
                onclick={() => onMoveDevice(entry.uid, -1)}
              >
                <ChevronUp size={16} />
              </button>
              <button
                type="button"
                class="btn btn-icon disabled:opacity-30"
                disabled={index === microphoneList.length - 1}
                aria-label={$t("settings_audio.move_down")}
                onclick={() => onMoveDevice(entry.uid, 1)}
              >
                <ChevronDown size={16} />
              </button>
              <label class="flex items-center gap-1.5 text-xs text-text-muted">
                <input
                  type="checkbox"
                  checked={entry.hidden}
                  class="switch scale-90"
                  aria-label={$t("settings_audio.hide_device")}
                  onchange={(event) =>
                    onToggleHidden(entry.uid, (event.currentTarget as HTMLInputElement).checked)}
                />
                {$t("settings_audio.hide_device")}
              </label>
            </div>
          </li>
        {/each}
      </ul>
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
