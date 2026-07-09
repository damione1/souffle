<script lang="ts">
  import { RefreshCw } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import SettingsField from "../../../components/ui/SettingsField.svelte";
  import type { AudioDeviceInfo } from "../../../types";

  const autostopMinutesOptions = [5, 10, 15, 30] as const;
  const autostopMinutesKeys: Record<(typeof autostopMinutesOptions)[number], string> = {
    5: "settings_audio.meeting_autostop_5min",
    10: "settings_audio.meeting_autostop_10min",
    15: "settings_audio.meeting_autostop_15min",
    30: "settings_audio.meeting_autostop_30min",
  };

  const maxDurationMinutesOptions = [120, 240, 480] as const;
  const maxDurationMinutesKeys: Record<(typeof maxDurationMinutesOptions)[number], string> = {
    120: "settings_audio.meeting_max_duration_2h",
    240: "settings_audio.meeting_max_duration_4h",
    480: "settings_audio.meeting_max_duration_8h",
  };

  let {
    audioDevices,
    selectedDevice,
    captureSystemAudio,
    systemAudioSupported,
    isLaptop,
    clamshellAudioDevice,
    vadEnabled,
    fillerRemoval,
    stutterCollapse,
    dictionaryCorrection,
    meetingAutostopEnabled,
    meetingAutostopMinutes,
    meetingMaxDurationMinutes,
    onDeviceChange,
    onRefreshDevices,
    onCaptureSystemAudioChange,
    onClamshellDeviceChange,
    onVadEnabledChange,
    onFillerRemovalChange,
    onStutterCollapseChange,
    onDictionaryCorrectionChange,
    onMeetingAutostopEnabledChange,
    onMeetingAutostopMinutesChange,
    onMeetingMaxDurationMinutesChange,
  }: {
    audioDevices: AudioDeviceInfo[];
    selectedDevice: string;
    captureSystemAudio: boolean;
    systemAudioSupported: boolean;
    isLaptop: boolean;
    clamshellAudioDevice: string | null;
    vadEnabled: boolean;
    fillerRemoval: boolean;
    stutterCollapse: boolean;
    dictionaryCorrection: boolean;
    meetingAutostopEnabled: boolean;
    meetingAutostopMinutes: number;
    meetingMaxDurationMinutes: number;
    onDeviceChange: (event: Event) => void | Promise<void>;
    onRefreshDevices: () => void | Promise<void>;
    onCaptureSystemAudioChange: (event: Event) => void | Promise<void>;
    onClamshellDeviceChange: (event: Event) => void | Promise<void>;
    onVadEnabledChange: (event: Event) => void | Promise<void>;
    onFillerRemovalChange: (event: Event) => void | Promise<void>;
    onStutterCollapseChange: (event: Event) => void | Promise<void>;
    onDictionaryCorrectionChange: (event: Event) => void | Promise<void>;
    onMeetingAutostopEnabledChange: (event: Event) => void | Promise<void>;
    onMeetingAutostopMinutesChange: (event: Event) => void | Promise<void>;
    onMeetingMaxDurationMinutesChange: (event: Event) => void | Promise<void>;
  } = $props();
</script>

<section class="settings-group">
  <h3>{$t("settings_audio.title")}</h3>
  <div class="settings-rows">
  <div class="flex items-center justify-between gap-4">
    <div class="flex min-w-0 flex-1 flex-col gap-0.5">
      <label for="input-device" class="setting-label">{$t("settings_audio.input_device")}</label>
      <span class="setting-desc">{$t("settings_audio.description")}</span>
    </div>
    <div class="flex shrink-0 gap-1.5 items-center">
      <select id="input-device" value={selectedDevice} onchange={onDeviceChange} class="field-select max-w-52">
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

  {#if isLaptop}
    <SettingsField
      label={$t("settings_audio.clamshell_device")}
      description={$t("settings_audio.clamshell_device_desc")}
      htmlFor="clamshell-device"
    >
      {#snippet control()}
        <select
          id="clamshell-device"
          value={clamshellAudioDevice ?? ""}
          onchange={onClamshellDeviceChange}
          class="field-select max-w-52"
        >
          <option value="">{$t("settings_audio.clamshell_device_follow_default")}</option>
          {#each audioDevices as device}
            <option value={device.name}>{device.name}</option>
          {/each}
        </select>
      {/snippet}
    </SettingsField>
  {/if}

  {#if systemAudioSupported}
    <SettingsField
      label={$t("settings_audio.capture_system_audio")}
      description={$t("settings_audio.capture_system_audio_desc")}
    >
      {#snippet control()}
        <input type="checkbox" checked={captureSystemAudio} onchange={onCaptureSystemAudioChange} class="switch" aria-label={$t("settings_audio.capture_system_audio")} />
      {/snippet}
    </SettingsField>
  {/if}

  <SettingsField
    label={$t("settings_audio.meeting_autostop")}
    description={$t("settings_audio.meeting_autostop_desc")}
  >
    {#snippet control()}
      <input type="checkbox" checked={meetingAutostopEnabled} onchange={onMeetingAutostopEnabledChange} class="switch" aria-label={$t("settings_audio.meeting_autostop")} />
    {/snippet}
  </SettingsField>

  {#if meetingAutostopEnabled}
    <SettingsField
      label={$t("settings_audio.meeting_autostop_minutes_label")}
      htmlFor="meeting-autostop-minutes"
    >
      {#snippet control()}
        <select
          id="meeting-autostop-minutes"
          value={meetingAutostopMinutes}
          onchange={onMeetingAutostopMinutesChange}
          class="field-select max-w-48"
        >
          {#each autostopMinutesOptions as minutes}
            <option value={minutes}>{$t(autostopMinutesKeys[minutes])}</option>
          {/each}
        </select>
      {/snippet}
    </SettingsField>

    <SettingsField
      label={$t("settings_audio.meeting_max_duration_label")}
      htmlFor="meeting-max-duration-minutes"
    >
      {#snippet control()}
        <select
          id="meeting-max-duration-minutes"
          value={meetingMaxDurationMinutes}
          onchange={onMeetingMaxDurationMinutesChange}
          class="field-select max-w-48"
        >
          {#each maxDurationMinutesOptions as minutes}
            <option value={minutes}>{$t(maxDurationMinutesKeys[minutes])}</option>
          {/each}
        </select>
      {/snippet}
    </SettingsField>
  {/if}

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
  </div>
</section>
