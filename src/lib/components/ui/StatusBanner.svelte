<script lang="ts">
  import { X } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  let {
    message,
    variant = "info",
    actionLabel,
    onAction,
    onDismiss
  }: {
    message: string;
    variant?: "warning" | "danger" | "info";
    actionLabel?: string;
    onAction?: () => void;
    onDismiss?: () => void;
  } = $props();
</script>

<div
  class={`rounded-default px-4 py-3 outline-1 flex items-center justify-between gap-4 ${
    variant === "warning"
      ? "outline-warning/30"
      : variant === "danger"
        ? "outline-danger/30"
        : "outline-ghost-border"
  }`}
>
  <p class="text-sm">{message}</p>

  <div class="flex items-center gap-2">
    {#if actionLabel && onAction}
      <button type="button" class="btn btn-primary btn-sm shrink-0" onclick={onAction}>
        {actionLabel}
      </button>
    {/if}
    {#if onDismiss}
      <button
        type="button"
        class="btn btn-icon shrink-0"
        onclick={onDismiss}
        aria-label={$t("mic_toast.dismiss")}
      >
        <X size={14} aria-hidden="true" />
      </button>
    {/if}
  </div>
</div>
