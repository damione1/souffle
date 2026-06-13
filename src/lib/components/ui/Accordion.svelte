<script lang="ts">
  import { ChevronRight } from "@lucide/svelte";
  import type { Snippet } from "svelte";
  import { slide } from "svelte/transition";

  let {
    title,
    open = $bindable(false),
    trailing,
    children,
  }: {
    title: string;
    open?: boolean;
    /** Optional content rendered on the right of the trigger (badges, count…). */
    trailing?: Snippet;
    children: Snippet;
  } = $props();
</script>

<div class="overflow-hidden rounded-card outline-1 outline-ghost-border bg-surface-2/70">
  <button
    onclick={() => (open = !open)}
    class="flex w-full cursor-pointer items-center gap-2.5 px-4 py-3 text-left transition-colors hover:bg-surface-3/50"
    aria-expanded={open}
  >
    <ChevronRight
      size={16}
      class={`shrink-0 text-text-muted transition-transform duration-200 ${open ? "rotate-90" : ""}`}
      aria-hidden="true"
    />
    <span class="font-heading text-sm font-semibold">{title}</span>
    <span class="flex flex-1 items-center justify-end gap-2">
      {#if trailing}{@render trailing()}{/if}
    </span>
  </button>

  {#if open}
    <div transition:slide={{ duration: 180 }} class="border-t border-ghost-border px-4 py-3.5">
      {@render children()}
    </div>
  {/if}
</div>
