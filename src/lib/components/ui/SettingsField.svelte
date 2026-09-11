<script lang="ts">
  import type { Snippet } from "svelte";
  import type { SettingsAnchor } from "../../features/settings/anchors";

  let {
    label,
    description,
    htmlFor,
    disabled = false,
    control,
    anchor,
  }: {
    /** Deep-link target: makes this row reachable by `openSettings({ anchor })`. */
    anchor?: SettingsAnchor;
    label: string;
    description?: string;
    htmlFor?: string;
    disabled?: boolean;
    control: Snippet;
  } = $props();
</script>

<div id={anchor} data-settings-anchor={anchor} class="flex items-center justify-between gap-4" class:opacity-50={disabled}>
  <div class="flex min-w-0 flex-1 flex-col gap-0.5">
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
