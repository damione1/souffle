<script lang="ts">
  import { X } from "@lucide/svelte";
  import type { Snippet } from "svelte";
  import { fade, fly } from "svelte/transition";

  let {
    title,
    onClose,
    children,
  }: {
    title: string;
    onClose: () => void;
    children: Snippet;
  } = $props();

  function onKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") onClose();
  }
</script>

<svelte:window onkeydown={onKeydown} />

<div
  class="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/40 p-6 pt-14 backdrop-blur-sm"
  onclick={(event) => {
    if (event.target === event.currentTarget) onClose();
  }}
  role="presentation"
  transition:fade={{ duration: 120 }}
>
  <div
    class="flex w-full max-w-2xl flex-col gap-4 rounded-2xl border border-ghost-border bg-canvas p-6 shadow-2xl"
    style="background: var(--color-surface-1);"
    role="dialog"
    aria-modal="true"
    aria-label={title}
    transition:fly={{ y: 16, duration: 180 }}
  >
    <div class="flex items-center justify-between gap-3">
      <h2>{title}</h2>
      <button
        onclick={onClose}
        class="cursor-pointer rounded-lg p-1.5 text-text-muted transition-colors hover:bg-surface-2 hover:text-text-primary"
        aria-label="Close"
      >
        <X size={18} aria-hidden="true" />
      </button>
    </div>
    {@render children()}
  </div>
</div>
