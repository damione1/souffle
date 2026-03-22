<script lang="ts">
  import type { Snippet } from "svelte";

  let { title, message, icon }: { title?: string; message: string; icon?: Snippet } = $props();
</script>

<div class="flex flex-col items-center gap-2 p-8 text-center text-text-muted">
  {#if icon}
    <div class="opacity-40">
      {@render icon()}
    </div>
  {/if}
  {#if title}
    <strong class="block text-text-secondary">{title}</strong>
  {/if}
  <p class="text-sm">{message}</p>
</div>
