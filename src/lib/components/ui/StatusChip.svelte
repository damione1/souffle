<script lang="ts">
  import { t } from "svelte-i18n";
  import type { TranscriptionRuntimePhase } from "../../types";
  import type { TranscriptionModelOperationState } from "../../features/transcription/state";

  let {
    phase,
    operationState,
    downloadedBytes = 0,
    downloadTotalBytes = null,
    modelLabel = "",
  }: {
    phase: TranscriptionRuntimePhase;
    operationState: TranscriptionModelOperationState;
    downloadedBytes?: number;
    downloadTotalBytes?: number | null;
    modelLabel?: string;
  } = $props();

  const percent = $derived(
    downloadTotalBytes ? Math.round((downloadedBytes / downloadTotalBytes) * 100) : null,
  );

  const status = $derived.by((): { key: string; tone: "ready" | "busy" | "attention" } => {
    if (operationState === "downloading") return { key: "status_chip.downloading", tone: "busy" };
    if (operationState === "loading" || phase === "load_required") {
      return { key: "status_chip.loading", tone: "busy" };
    }
    if (phase === "ready") return { key: "status_chip.ready", tone: "ready" };
    return { key: "status_chip.model_required", tone: "attention" };
  });
</script>

<span
  class="inline-flex items-center gap-2 rounded-full bg-surface-2 px-[11px] py-[5px] text-xs text-text-tertiary outline-1 outline-ghost-border"
  role="status"
>
  {#if status.tone === "ready"}
    <span class="ready-dot" aria-hidden="true"></span>
  {:else}
    <span
      class={`h-[7px] w-[7px] rounded-full ${
        status.tone === "busy" ? "animate-pulse bg-accent" : "bg-warning"
      }`}
      aria-hidden="true"
    ></span>
  {/if}
  {$t(status.key)}{status.key === "status_chip.downloading" && percent !== null ? ` ${percent}%` : ""}
  {#if status.tone === "ready" && modelLabel}
    <span class="font-mono text-[11px] text-text-muted">{modelLabel}</span>
  {/if}
</span>
