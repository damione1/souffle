<script lang="ts">
  import { t } from "svelte-i18n";
  import SettingsField from "../../../components/ui/SettingsField.svelte";
  import StatusBanner from "../../../components/ui/StatusBanner.svelte";
  import { SUPPORTED_LOCALES } from "../../../i18n";
  import type { Theme } from "../../../types";

  const themeOptions: Theme[] = ["dark", "light", "system"];
  const themeKeys: Record<Theme, string> = {
    dark: "settings_interface.theme_dark",
    light: "settings_interface.theme_light",
    system: "settings_interface.theme_system",
  };

  let {
    theme,
    locale,
    autoPaste,
    pasteDelayMs,
    toggleShortcut,
    pttShortcut,
    recordingField,
    shortcutError,
    onThemeChange,
    onLocaleChange,
    onAutoPasteChange,
    onPasteDelayChange,
    onStartRecording,
    onClearShortcut,
    formatShortcut,
  }: {
    theme: Theme;
    locale: string;
    autoPaste: boolean;
    pasteDelayMs: number;
    toggleShortcut: string;
    pttShortcut: string;
    recordingField: "toggle" | "ptt" | null;
    shortcutError: string;
    onThemeChange: (theme: Theme) => void;
    onLocaleChange: (locale: string) => void;
    onAutoPasteChange: (event: Event) => void;
    onPasteDelayChange: (event: Event) => void;
    onStartRecording: (field: "toggle" | "ptt") => void;
    onClearShortcut: (field: "toggle" | "ptt") => void | Promise<void>;
    formatShortcut: (shortcut: string) => string;
  } = $props();
</script>

<section class="surface-card flex flex-col gap-3.5">
  <h3>{$t("settings_interface.title")}</h3>

  <SettingsField
    label={$t("settings_interface.language")}
    description={$t("settings_interface.language_desc")}
  >
    {#snippet control()}
      <select
        value={locale || "en"}
        onchange={(event) => onLocaleChange((event.currentTarget as HTMLSelectElement).value)}
        class="field-select max-w-48"
      >
        {#each SUPPORTED_LOCALES as loc}
          <option value={loc.id}>{loc.label}</option>
        {/each}
      </select>
    {/snippet}
  </SettingsField>

  <SettingsField label={$t("settings_interface.theme")}>
    {#snippet control()}
      <div class="flex gap-1">
        {#each themeOptions as themeOption}
          <button
            onclick={() => onThemeChange(themeOption)}
            class={`btn ${theme === themeOption ? "btn-active" : ""}`}
          >
            {$t(themeKeys[themeOption])}
          </button>
        {/each}
      </div>
    {/snippet}
  </SettingsField>

  <SettingsField
    label={$t("settings_interface.auto_paste")}
    description={$t("settings_interface.auto_paste_desc")}
  >
    {#snippet control()}
      <input
        type="checkbox"
        checked={autoPaste}
        onchange={onAutoPasteChange}
        class="switch"
        aria-label={$t("settings_interface.auto_paste")}
      />
    {/snippet}
  </SettingsField>

  {#if autoPaste}
    <SettingsField
      label={$t("settings_interface.paste_delay")}
      description={$t("settings_interface.paste_delay_desc")}
      htmlFor="paste-delay"
    >
      {#snippet control()}
        <input
          id="paste-delay"
          type="number"
          value={pasteDelayMs}
          onchange={onPasteDelayChange}
          min="50"
          max="1000"
          step="50"
          class="field-number"
        />
      {/snippet}
    </SettingsField>
  {/if}

  <SettingsField
    label={$t("settings_interface.toggle_recording")}
    description={$t("settings_interface.toggle_recording_desc")}
  >
    {#snippet control()}
      <div class="flex gap-2 items-center">
        <button
          onclick={() => onStartRecording("toggle")}
          class="shortcut-button"
          class:is-recording={recordingField === "toggle"}
        >
          {recordingField === "toggle" ? $t("settings_interface.press_keys") : formatShortcut(toggleShortcut)}
        </button>
        {#if toggleShortcut}
          <button onclick={() => onClearShortcut("toggle")} class="btn btn-ghost text-sm">{$t("settings_interface.clear")}</button>
        {/if}
      </div>
    {/snippet}
  </SettingsField>

  <SettingsField
    label={$t("settings_interface.push_to_talk")}
    description={$t("settings_interface.push_to_talk_desc")}
  >
    {#snippet control()}
      <div class="flex gap-2 items-center">
        <button
          onclick={() => onStartRecording("ptt")}
          class="shortcut-button"
          class:is-recording={recordingField === "ptt"}
        >
          {recordingField === "ptt" ? $t("settings_interface.press_keys") : formatShortcut(pttShortcut)}
        </button>
        {#if pttShortcut}
          <button onclick={() => onClearShortcut("ptt")} class="btn btn-ghost text-sm">{$t("settings_interface.clear")}</button>
        {/if}
      </div>
    {/snippet}
  </SettingsField>

  {#if shortcutError}
    <StatusBanner message={shortcutError} variant="danger" />
  {/if}
</section>
