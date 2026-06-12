<script lang="ts">
  import { t } from "svelte-i18n";
  import type { TranscriptionRuntimePhase } from "../../types";
  import type { TranscriptionModelOperationState } from "../../features/transcription/state";

  let {
    phase,
    operationState,
    downloadedBytes = 0,
    downloadTotalBytes = null,
  }: {
    phase: TranscriptionRuntimePhase;
    operationState: TranscriptionModelOperationState;
    downloadedBytes?: number;
    downloadTotalBytes?: number | null;
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
  class="inline-flex items-center gap-1.5 rounded-full bg-surface-2 px-2.5 py-1 text-xs text-text-secondary"
  role="status"
>
  <span
    class={`h-1.5 w-1.5 rounded-full ${
      status.tone === "ready"
        ? "bg-emerald-400"
        : status.tone === "busy"
          ? "animate-pulse bg-accent-blue"
          : "bg-amber-400"
    }`}
    aria-hidden="true"
  ></span>
  {$t(status.key)}{status.key === "status_chip.downloading" && percent !== null ? ` ${percent}%` : ""}
</span>
