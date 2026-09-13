<script lang="ts">
  import { t } from "svelte-i18n";
  import {
    updateController,
    useUpdateController,
  } from "./controller.svelte";
  import type { InstallBlockReason } from "../../api/updater";

  let {
    releaseUrl = null,
    compact = false,
  }: {
    releaseUrl?: string | null;
    /** Compact link-style for Settings; full buttons for the dialog. */
    compact?: boolean;
  } = $props();

  useUpdateController();

  const status = $derived(updateController.status);
  const phase = $derived(status.phase);
  const busy = $derived(updateController.busy);
  const installBlock = $derived(updateController.installBlock);
  const actionError = $derived(updateController.actionError ?? status.error);

  const progressPct = $derived.by(() => {
    if (!status.total_bytes || status.total_bytes <= 0) return null;
    return Math.min(100, Math.round((status.downloaded_bytes / status.total_bytes) * 100));
  });

  function blockLabel(reason: InstallBlockReason): string {
    return $t(`update_available.blocked_${reason}`);
  }

  const btnClass = $derived(compact ? "text-xs text-accent hover:underline cursor-pointer" : "btn btn-active");
  const ghostClass = $derived(compact ? "text-xs text-text-muted hover:underline cursor-pointer" : "btn");
</script>

<div class="flex flex-col {compact ? 'items-end gap-1' : 'gap-2'}">
  {#if phase === "idle" || (phase === "failed" && !status.manual_fallback)}
    <button
      type="button"
      class={btnClass}
      disabled={busy}
      onclick={() => void updateController.download()}
    >
      {$t("update_available.download_update")}
    </button>
  {:else if phase === "downloading"}
    <div class="flex {compact ? 'flex-col items-end' : 'items-center'} gap-2 w-full">
      {#if progressPct !== null}
        <div
          class="h-1.5 flex-1 min-w-[8rem] overflow-hidden rounded-full bg-ghost-border"
          role="progressbar"
          aria-valuenow={progressPct}
          aria-valuemin={0}
          aria-valuemax={100}
        >
          <div class="h-full bg-accent transition-[width]" style="width: {progressPct}%"></div>
        </div>
        <span class="text-xs text-text-muted tabular-nums">{progressPct}%</span>
      {:else}
        <span class="text-xs text-text-muted">{$t("update_available.downloading")}</span>
      {/if}
      <button
        type="button"
        class={ghostClass}
        disabled={busy}
        onclick={() => void updateController.cancel()}
      >
        {$t("update_available.cancel")}
      </button>
    </div>
  {:else if phase === "ready"}
    <button
      type="button"
      class={btnClass}
      disabled={busy || installBlock !== null}
      title={installBlock ? blockLabel(installBlock) : undefined}
      onclick={() => void updateController.install()}
    >
      {$t("update_available.install_restart")}
    </button>
    {#if installBlock}
      <p class="text-xs text-text-muted">{blockLabel(installBlock)}</p>
    {/if}
  {:else if phase === "failed"}
    {#if actionError}
      <p class="text-xs text-danger-soft {compact ? 'text-right' : ''}">{actionError}</p>
    {/if}
    <div class="flex {compact ? 'flex-col items-end' : 'justify-end'} gap-2">
      <button
        type="button"
        class={btnClass}
        disabled={busy}
        onclick={() => void updateController.download()}
      >
        {$t("update_available.retry")}
      </button>
      {#if releaseUrl}
        <button
          type="button"
          class={ghostClass}
          onclick={() => updateController.openFallback(releaseUrl)}
        >
          {$t("update_available.open_release_page")}
        </button>
      {/if}
    </div>
  {/if}

  {#if phase !== "failed" && status.manual_fallback && releaseUrl}
    <button
      type="button"
      class={ghostClass}
      onclick={() => updateController.openFallback(releaseUrl)}
    >
      {$t("update_available.open_release_page")}
    </button>
  {/if}
</div>
