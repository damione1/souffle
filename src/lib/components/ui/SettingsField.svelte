<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    label,
    description,
    htmlFor,
    disabled = false,
    control,
  }: {
    label: string;
    description?: string;
    htmlFor?: string;
    disabled?: boolean;
    control: Snippet;
  } = $props();
</script>

<div class="flex items-center justify-between gap-4" class:opacity-50={disabled}>
  <div>
    {#if htmlFor}
      <label for={htmlFor} class="setting-label">{label}</label>
    {:else}
      <span class="setting-label">{label}</span>
    {/if}
    {#if description}
      <span class="setting-desc">{description}</span>
    {/if}
  </div>
  {@render control()}
</div>
