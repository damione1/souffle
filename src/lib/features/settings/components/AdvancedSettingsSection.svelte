<script lang="ts">
  import { ChevronRight } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import type { Snippet } from "svelte";

  let { children }: { children: Snippet } = $props();

  let open = $state(false);
</script>

<section class="flex flex-col gap-[22px]">
  <button
    onclick={() => (open = !open)}
    class="flex cursor-pointer items-center gap-1.5 px-0.5 text-left text-[11px] font-semibold uppercase tracking-[0.15em] text-text-muted transition-colors hover:text-text-secondary"
    aria-expanded={open}
  >
    <ChevronRight
      size={14}
      class={`transition-transform ${open ? "rotate-90" : ""}`}
      aria-hidden="true"
    />
    {$t("settings_advanced.title")}
  </button>

  {#if open}
    <p class="-mt-4 px-0.5 text-xs text-text-muted">{$t("settings_advanced.description")}</p>
    {@render children()}
  {/if}
</section>
