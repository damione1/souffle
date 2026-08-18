<script lang="ts">
  import { X } from "@lucide/svelte";
  import { t } from "svelte-i18n";

  let {
    title,
    detail,
    hint,
    onDismiss,
  }: {
    title: string;
    detail?: string;
    hint?: string;
    onDismiss: () => void;
  } = $props();
</script>

<div
  role="status"
  class="flex max-w-sm items-start gap-2.5 rounded-xl bg-black/75 px-3.5 py-2.5 text-sm text-white shadow-lg backdrop-blur-md"
>
  <div class="min-w-0 flex-1">
    <p class="font-medium">{title}</p>
    {#if detail}
      <p class="truncate text-white/80">{detail}</p>
    {/if}
    {#if hint}
      <p class="mt-0.5 text-xs text-white/55">{hint}</p>
    {/if}
  </div>
  <button
    type="button"
    class="shrink-0 rounded-md p-0.5 text-white/60 transition-colors hover:bg-white/10 hover:text-white"
    aria-label={$t("mic_toast.dismiss")}
    onclick={onDismiss}
  >
    <X size={14} aria-hidden="true" />
  </button>
</div>
