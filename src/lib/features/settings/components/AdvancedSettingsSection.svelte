<script lang="ts">
  import { ChevronRight } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import type { Snippet } from "svelte";

  let { children }: { children: Snippet } = $props();

  let open = $state(false);
</script>

<section class="flex flex-col gap-4">
  <button
    onclick={() => (open = !open)}
    class="flex items-center gap-2 text-left text-sm font-medium text-text-muted hover:text-text-secondary transition-colors"
    aria-expanded={open}
  >
    <ChevronRight
      size={16}
      class={`transition-transform ${open ? "rotate-90" : ""}`}
      aria-hidden="true"
    />
    {$t("settings_advanced.title")}
  </button>

  {#if open}
    <div class="flex flex-col gap-4">
      <p class="text-xs text-text-muted -mt-2">{$t("settings_advanced.description")}</p>
      {@render children()}
    </div>
  {/if}
</section>
