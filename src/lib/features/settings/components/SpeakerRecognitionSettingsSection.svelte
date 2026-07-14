<script lang="ts">
  import { t } from "svelte-i18n";
  import ProgressBar from "../../../components/ui/ProgressBar.svelte";
  import SettingsField from "../../../components/ui/SettingsField.svelte";

  const diarizeMaxSpeakersOptions = [2, 3, 4, 5, 6, 8, 10, 12, 15, 20] as const;

  let {
    captureSystemAudio,
    diarizeEnabled,
    diarizeMic,
    diarizeSystemAudio,
    diarizeMaxSpeakers,
    diarizeDownloadState,
    diarizeDownloadedBytes,
    diarizeDownloadTotalBytes,
    onDiarizeEnabledChange,
    onDiarizeMicChange,
    onDiarizeSystemAudioChange,
    onDiarizeMaxSpeakersChange,
  }: {
    captureSystemAudio: boolean;
    diarizeEnabled: boolean;
    diarizeMic: boolean;
    diarizeSystemAudio: boolean;
    diarizeMaxSpeakers: number | null;
    diarizeDownloadState: "idle" | "downloading" | "error";
    diarizeDownloadedBytes: number;
    diarizeDownloadTotalBytes: number | null;
    onDiarizeEnabledChange: (event: Event) => void | Promise<void>;
    onDiarizeMicChange: (event: Event) => void | Promise<void>;
    onDiarizeSystemAudioChange: (event: Event) => void | Promise<void>;
    onDiarizeMaxSpeakersChange: (event: Event) => void | Promise<void>;
  } = $props();
</script>

<section class="settings-group">
  <h3>{$t("settings_speakers.title")}</h3>
  <div class="settings-rows">
    <SettingsField
      label={$t("settings_speakers.enabled")}
      description={$t("settings_speakers.enabled_desc")}
    >
      {#snippet control()}
        <input
          type="checkbox"
          checked={diarizeEnabled}
          disabled={diarizeDownloadState === "downloading"}
          onchange={onDiarizeEnabledChange}
          class="switch"
          aria-label={$t("settings_speakers.enabled")}
        />
      {/snippet}
    </SettingsField>

    {#if diarizeDownloadState === "downloading"}
      <div>
        <ProgressBar
          value={diarizeDownloadedBytes}
          max={diarizeDownloadTotalBytes && diarizeDownloadTotalBytes > 0 ? diarizeDownloadTotalBytes : 100}
          label={$t("settings_speakers.downloading")}
        />
      </div>
    {/if}

    {#if diarizeEnabled}
      <SettingsField
        label={$t("settings_speakers.mic")}
        description={$t("settings_speakers.mic_desc")}
      >
        {#snippet control()}
          <input
            type="checkbox"
            checked={diarizeMic}
            disabled={diarizeDownloadState === "downloading"}
            onchange={onDiarizeMicChange}
            class="switch"
            aria-label={$t("settings_speakers.mic")}
          />
        {/snippet}
      </SettingsField>

      <SettingsField
        label={$t("settings_speakers.system_audio")}
        description={captureSystemAudio
          ? $t("settings_speakers.system_audio_desc")
          : $t("settings_speakers.system_audio_requires_capture")}
      >
        {#snippet control()}
          <input
            type="checkbox"
            checked={diarizeSystemAudio}
            disabled={!captureSystemAudio || diarizeDownloadState === "downloading"}
            onchange={onDiarizeSystemAudioChange}
            class="switch"
            aria-label={$t("settings_speakers.system_audio")}
          />
        {/snippet}
      </SettingsField>

      <SettingsField
        label={$t("settings_speakers.max_speakers_label")}
        description={$t("settings_speakers.max_speakers_desc")}
        htmlFor="diarize-max-speakers"
      >
        {#snippet control()}
          <select
            id="diarize-max-speakers"
            value={diarizeMaxSpeakers ?? ""}
            onchange={onDiarizeMaxSpeakersChange}
            class="field-select max-w-48"
          >
            <option value="">{$t("settings_speakers.max_speakers_auto")}</option>
            {#each diarizeMaxSpeakersOptions as count}
              <option value={count}>{count}</option>
            {/each}
          </select>
        {/snippet}
      </SettingsField>
    {/if}
  </div>
</section>
