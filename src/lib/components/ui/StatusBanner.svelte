<script lang="ts">
  let { message, variant = "info" }: { message: string; variant?: "warning" | "danger" | "info" } = $props();
</script>

<div
  class={`rounded-default bg-surface-3 px-4 py-3 outline-1 ${
    variant === "warning"
      ? "outline-warning/30"
      : variant === "danger"
        ? "outline-danger/30"
        : "outline-ghost-border"
  }`}
>
  <p class="text-sm">{message}</p>
</div>
