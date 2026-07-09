<script lang="ts">
  import { Download, FolderOpen } from "@lucide/svelte";
  import { onDestroy, onMount } from "svelte";
  import { t } from "svelte-i18n";
  import ProgressBar from "../../../components/ui/ProgressBar.svelte";
  import SettingsField from "../../../components/ui/SettingsField.svelte";
  import { exportArchive, getDataStats, revealDataDir } from "../../../api/data";
  import { events } from "../../../api/generated";
  import type { ArchiveExportProgress, DataStats } from "../../../types";
  import { errorMessage, formatBytes } from "../../../utils";

  let stats = $state<DataStats | null>(null);
  let exporting = $state(false);
  let progress = $state<ArchiveExportProgress | null>(null);
  let statusMessage = $state("");
  let statusIsError = $state(false);

  let unlistenProgress: (() => void) | null = null;

  async function refreshStats() {
    try {
      stats = await getDataStats();
    } catch (e) {
      statusMessage = errorMessage(e);
      statusIsError = true;
    }
  }

  onMount(() => {
    void refreshStats();

    events.archiveExportProgress.listen((event) => {
      progress = event.payload;
      if (!event.payload.finished) return;

      exporting = false;
      if (event.payload.error) {
        statusMessage = event.payload.error;
        statusIsError = true;
      } else {
        statusMessage = $t("settings_data.export_success");
        statusIsError = false;
        void refreshStats();
      }
    }).then((fn) => {
      unlistenProgress = fn;
    });
  });

  onDestroy(() => {
    unlistenProgress?.();
  });

  /** Directory picker from the dialog plugin is only needed for this one
   * action, so it is dynamically imported rather than pulled into the main
   * settings bundle up front. */
  async function handleExport() {
    statusMessage = "";
    statusIsError = false;
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const dir = await open({ directory: true, multiple: false });
      if (!dir || typeof dir !== "string") return; // user cancelled the dialog

      progress = null;
      exporting = true;
      await exportArchive(dir);
    } catch (e) {
      exporting = false;
      statusMessage = errorMessage(e);
      statusIsError = true;
    }
  }

  async function handleReveal() {
    try {
      await revealDataDir();
    } catch (e) {
      statusMessage = errorMessage(e);
      statusIsError = true;
    }
  }
</script>

<section class="settings-group">
  <h3>{$t("settings_data.title")}</h3>
  <div class="settings-rows">
    {#if stats}
      <p class="setting-desc">
        {$t("settings_data.stats", {
          values: {
            size: formatBytes(stats.db_size_bytes),
            meetings: stats.meeting_count,
            dictations: stats.dictation_count,
          },
        })}
      </p>
    {/if}

    <SettingsField label={$t("settings_data.export_title")} description={$t("settings_data.export_desc")}>
      {#snippet control()}
        <button class="btn btn-ghost gap-[7px]" disabled={exporting} onclick={handleExport}>
          <Download size={14} aria-hidden="true" />
          {exporting ? $t("settings_data.exporting") : $t("settings_data.export_button")}
        </button>
      {/snippet}
    </SettingsField>

    {#if exporting && progress}
      <ProgressBar value={progress.done} max={progress.total} label={$t("settings_data.exporting")} />
    {/if}

    <SettingsField label={$t("settings_data.reveal_title")} description={$t("settings_data.reveal_desc")}>
      {#snippet control()}
        <button class="btn btn-ghost gap-[7px]" onclick={handleReveal}>
          <FolderOpen size={14} aria-hidden="true" />
          {$t("settings_data.reveal_button")}
        </button>
      {/snippet}
    </SettingsField>

    {#if statusMessage}
      <p class={`setting-desc ${statusIsError ? "!text-danger-strong" : ""}`} role={statusIsError ? "alert" : "status"}>
        {statusMessage}
      </p>
    {/if}
  </div>
</section>
