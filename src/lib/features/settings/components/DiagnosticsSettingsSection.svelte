<script lang="ts">
  import { ClipboardCopy } from "@lucide/svelte";
  import { onDestroy, onMount } from "svelte";
  import { t } from "svelte-i18n";
  import CopyButton from "../../../components/ui/CopyButton.svelte";
  import SettingsField from "../../../components/ui/SettingsField.svelte";
  import { getDiagnosticsText, getLogTail } from "../../../api/diagnostics";
  import type { LogLevel } from "../../../types";
  import { errorMessage } from "../../../utils";

  const LOG_LEVELS: LogLevel[] = ["error", "warn", "info", "debug", "trace"];
  const TAIL_LINES = 80;
  const POLL_MS = 2000;

  let {
    debugTranscription,
    logLevel,
    onDebugTranscriptionChange,
    onLogLevelChange,
  }: {
    debugTranscription: boolean;
    logLevel: LogLevel;
    onDebugTranscriptionChange: (event: Event) => void;
    onLogLevelChange: (event: Event) => void;
  } = $props();

  let logTail = $state("");
  let tailError = $state("");
  let copying = $state(false);
  let pollTimer: ReturnType<typeof setInterval> | null = null;

  async function refreshTail() {
    try {
      logTail = await getLogTail(TAIL_LINES);
      tailError = "";
    } catch (e) {
      tailError = errorMessage(e);
    }
  }

  async function copyDiagnostics() {
    copying = true;
    try {
      const text = await getDiagnosticsText();
      await navigator.clipboard.writeText(text);
    } catch (e) {
      tailError = errorMessage(e);
    } finally {
      copying = false;
    }
  }

  onMount(() => {
    void refreshTail();
    pollTimer = setInterval(() => void refreshTail(), POLL_MS);
  });

  onDestroy(() => {
    if (pollTimer) clearInterval(pollTimer);
  });
</script>

<section class="settings-group">
  <h3>{$t("settings_diagnostics.title")}</h3>
  <div class="settings-rows">
    <SettingsField
      label={$t("settings_diagnostics.log_level")}
      description={$t("settings_diagnostics.log_level_desc")}
    >
      {#snippet control()}
        <select
          value={logLevel}
          onchange={onLogLevelChange}
          class="field-select max-w-40"
          aria-label={$t("settings_diagnostics.log_level")}
        >
          {#each LOG_LEVELS as level}
            <option value={level}>{$t(`settings_diagnostics.log_level_${level}`)}</option>
          {/each}
        </select>
      {/snippet}
    </SettingsField>

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

    <SettingsField
      label={$t("settings_diagnostics.log_viewer")}
      description={$t("settings_diagnostics.log_viewer_desc")}
    >
      {#snippet control()}
        <div class="flex w-full flex-col gap-2">
          <pre
            class="max-h-48 overflow-auto rounded-lg border border-ghost-border bg-surface-1/80 p-3 font-mono text-[11px] leading-relaxed text-text-secondary"
            aria-live="polite"
          >{logTail || $t("settings_diagnostics.log_empty")}</pre>
          {#if tailError}
            <p class="text-xs text-danger-soft">{tailError}</p>
          {/if}
        </div>
      {/snippet}
    </SettingsField>

    <div class="flex justify-end">
      <button
        onclick={() => void copyDiagnostics()}
        class="btn btn-ghost gap-1.5 px-2.5 py-[5px] text-[12.5px]"
        disabled={copying}
      >
        <ClipboardCopy size={14} aria-hidden="true" />
        {copying ? $t("settings_diagnostics.copying") : $t("settings_diagnostics.copy_diagnostics")}
      </button>
    </div>
  </div>
</section>
