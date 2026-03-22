<script lang="ts">
  let {
    label,
    confirmLabel = "Yes",
    confirmMessage = "Are you sure?",
    variant = "danger",
    onConfirm,
  }: {
    label: string;
    confirmLabel?: string;
    confirmMessage?: string;
    variant?: "danger" | "ghost";
    onConfirm: () => void;
  } = $props();

  let confirming = $state(false);
</script>

{#if confirming}
  <div class="flex items-center gap-2 flex-wrap">
    <span class="text-sm text-text-muted">{confirmMessage}</span>
    <button
      onclick={() => {
        onConfirm();
        confirming = false;
      }}
      class={`btn ${variant === "danger" ? "btn-danger" : ""}`}
    >
      {confirmLabel}
    </button>
    <button onclick={() => (confirming = false)} class="btn btn-ghost">Cancel</button>
  </div>
{:else}
  <button
    onclick={() => (confirming = true)}
    class={`btn btn-ghost ${variant === "danger" ? "text-danger" : ""}`}
  >
    {label}
  </button>
{/if}
