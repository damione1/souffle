<script lang="ts">
  let {
    message,
    variant = "info",
    actionLabel,
    onAction
  }: {
    message: string;
    variant?: "warning" | "danger" | "info";
    actionLabel?: string;
    onAction?: () => void;
  } = $props();
</script>

<div
  class={`rounded-default bg-surface-3 px-4 py-3 outline-1 flex items-center justify-between gap-4 ${
    variant === "warning"
      ? "outline-warning/30"
      : variant === "danger"
        ? "outline-danger/30"
        : "outline-ghost-border"
  }`}
>
  <p class="text-sm">{message}</p>
  {#if actionLabel && onAction}
    <button class="btn btn-primary btn-sm shrink-0" onclick={onAction}>
      {actionLabel}
    </button>
  {/if}
</div>
