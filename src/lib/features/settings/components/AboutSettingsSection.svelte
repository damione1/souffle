<script lang="ts">
  import { onMount } from "svelte";
  import { t } from "svelte-i18n";
  import { checkForUpdates, getAppVersion, openReleasePage } from "../../../api/diagnostics";
  import type { UpdateCheckResult } from "../../../types";
  import { errorMessage } from "../../../utils";

  let {
    selectedTranscriptionLabel,
    selectedOllamaModelLabel,
  }: {
    selectedTranscriptionLabel: string;
    selectedOllamaModelLabel: string;
  } = $props();

  let appVersion = $state("");
  let checking = $state(false);
  let updateResult = $state<UpdateCheckResult | null>(null);
  let statusMessage = $state("");
  let statusIsError = $state(false);

  onMount(() => {
    void getAppVersion().then((v) => {
      appVersion = v;
    });
  });

  async function handleCheckUpdates() {
    checking = true;
    statusMessage = "";
    statusIsError = false;
    try {
      updateResult = await checkForUpdates();
      if (updateResult.check_error) {
        statusMessage = updateResult.check_error;
        statusIsError = true;
      } else if (updateResult.update_available) {
        statusMessage = $t("settings_about.update_available", {
          values: { version: updateResult.latest_version ?? "" },
        });
      } else {
        statusMessage = $t("settings_about.up_to_date");
      }
    } catch (e) {
      statusMessage = errorMessage(e);
      statusIsError = true;
    } finally {
      checking = false;
    }
  }
</script>

<section class="settings-group">
  <h3>{$t("settings_about.title")}</h3>
  <div class="settings-rows">
    <div class="flex justify-between gap-4">
      <span class="setting-desc">{$t("settings_about.version")}</span>
      <span class="text-[13px] text-text-secondary">v{appVersion || "…"}</span>
    </div>
    <div class="flex justify-between gap-4">
      <span class="setting-desc">{$t("settings_about.transcription")}</span>
      <span class="text-[13px] text-right text-text-secondary">{selectedTranscriptionLabel}</span>
    </div>
    <div class="flex justify-between gap-4">
      <span class="setting-desc">{$t("settings_about.summaries")}</span>
      <span class="text-[13px] text-right text-text-secondary">{selectedOllamaModelLabel || $t("settings_about.not_configured")}</span>
    </div>
    <div class="flex justify-between gap-4">
      <span class="setting-desc">{$t("settings_about.privacy")}</span>
      <span class="text-[13px] text-text-secondary">{$t("settings_about.privacy_value")}</span>
    </div>
    <div class="flex items-center justify-between gap-4">
      <span class="setting-desc">{$t("settings_about.updates")}</span>
      <div class="flex flex-col items-end gap-1">
        <button
          onclick={() => void handleCheckUpdates()}
          class="btn btn-ghost text-[12.5px]"
          disabled={checking}
        >
          {checking ? $t("settings_about.checking") : $t("settings_about.check_updates")}
        </button>
        {#if updateResult?.update_available && updateResult.release_url}
          <button
            onclick={() => void openReleasePage(updateResult!.release_url!)}
            class="text-xs text-accent hover:underline cursor-pointer"
          >
            {$t("settings_about.download_update")}
          </button>
        {/if}
      </div>
    </div>
    {#if statusMessage}
      <p class={`text-xs ${statusIsError ? "text-danger-soft" : "text-text-muted"}`}>
        {statusMessage}
      </p>
    {/if}
  </div>
</section>
