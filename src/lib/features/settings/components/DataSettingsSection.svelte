<script lang="ts">
  import { Download, FolderOpen, PlugZap } from "@lucide/svelte";
  import { onDestroy, onMount } from "svelte";
  import { t } from "svelte-i18n";
  import CopyButton from "../../../components/ui/CopyButton.svelte";
  import ProgressBar from "../../../components/ui/ProgressBar.svelte";
  import SettingsField from "../../../components/ui/SettingsField.svelte";
  import {
    exportArchive,
    getDataStats,
    getMcpSetupInfo,
    revealDataDir,
    testMcpConnection,
  } from "../../../api/data";
  import { events } from "../../../api/generated";
  import type { ArchiveExportProgress, DataStats, McpSetupInfo } from "../../../types";
  import { errorMessage, formatBytes } from "../../../utils";

  let stats = $state<DataStats | null>(null);
  let mcpSetup = $state<McpSetupInfo | null>(null);
  let exporting = $state(false);
  let testingMcp = $state(false);
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

  async function refreshMcpSetup() {
    try {
      mcpSetup = await getMcpSetupInfo();
    } catch (e) {
      statusMessage = errorMessage(e);
      statusIsError = true;
    }
  }

  onMount(() => {
    void refreshStats();
    void refreshMcpSetup();

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

  async function handleTestMcp() {
    statusMessage = "";
    statusIsError = false;
    testingMcp = true;
    try {
      const tools = await testMcpConnection();
      statusMessage = $t("settings_data.mcp_test_success", { values: { tools } });
      statusIsError = false;
    } catch (e) {
      statusMessage = errorMessage(e);
      statusIsError = true;
    } finally {
      testingMcp = false;
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

    <div class="flex flex-col gap-3 border-t border-border/60 pt-4">
      <div class="flex flex-col gap-0.5">
        <span class="setting-label">{$t("settings_data.mcp_title")}</span>
        <span class="setting-desc">{$t("settings_data.mcp_desc")}</span>
      </div>

      {#if mcpSetup}
        <div class="flex flex-col gap-1.5">
          <span class="setting-label">{$t("settings_data.mcp_binary")}</span>
          <code class="break-all rounded-md bg-surface-raised px-2.5 py-2 font-mono text-[11px] text-text-muted">
            {mcpSetup.binary_path}
          </code>
          {#if !mcpSetup.exists}
            <p class="setting-desc text-warning">{$t("settings_data.mcp_binary_missing")}</p>
          {/if}
        </div>

        <div class="flex flex-col gap-1.5">
          <div class="flex items-center justify-between gap-3">
            <div class="flex min-w-0 flex-1 flex-col gap-0.5">
              <span class="setting-label">{$t("settings_data.mcp_claude_desktop")}</span>
              <span class="setting-desc">{$t("settings_data.mcp_claude_desktop_desc")}</span>
            </div>
            <CopyButton text={mcpSetup.claude_desktop_snippet} />
          </div>
          <pre class="max-h-36 overflow-auto rounded-md bg-surface-raised px-2.5 py-2 font-mono text-[11px] text-text-muted">{mcpSetup.claude_desktop_snippet}</pre>
        </div>

        <div class="flex flex-col gap-1.5">
          <div class="flex items-center justify-between gap-3">
            <div class="flex min-w-0 flex-1 flex-col gap-0.5">
              <span class="setting-label">{$t("settings_data.mcp_claude_code")}</span>
              <span class="setting-desc">{$t("settings_data.mcp_claude_code_desc")}</span>
            </div>
            <CopyButton text={mcpSetup.claude_code_command} />
          </div>
          <code class="break-all rounded-md bg-surface-raised px-2.5 py-2 font-mono text-[11px] text-text-muted">
            {mcpSetup.claude_code_command}
          </code>
        </div>

        {@const mcpExists = mcpSetup.exists}
        <SettingsField label={$t("settings_data.mcp_test")} disabled={!mcpExists}>
          {#snippet control()}
            <button
              class="btn btn-ghost gap-[7px]"
              disabled={!mcpExists || testingMcp}
              onclick={handleTestMcp}
            >
              <PlugZap size={14} aria-hidden="true" />
              {testingMcp ? $t("settings_data.mcp_testing") : $t("settings_data.mcp_test")}
            </button>
          {/snippet}
        </SettingsField>
      {/if}
    </div>

    {#if statusMessage}
      <p class={`setting-desc ${statusIsError ? "!text-danger-strong" : ""}`} role={statusIsError ? "alert" : "status"}>
        {statusMessage}
      </p>
    {/if}
  </div>
</section>
