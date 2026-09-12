<script lang="ts">
  import { X } from "@lucide/svelte";
  import { t } from "svelte-i18n";

  let {
    title,
    detail,
    hint,
    actionLabel,
    onAction,
    onDismiss,
  }: {
    title: string;
    detail?: string;
    hint?: string;
    actionLabel?: string;
    onAction?: () => void;
    onDismiss: () => void;
  } = $props();
</script>

<div
  role="status"
  class="flex max-w-sm items-start gap-2.5 rounded-default bg-black/75 px-3.5 py-2.5 text-sm text-white backdrop-blur-md"
>
  <div class="min-w-0 flex-1">
    <p class="font-medium">{title}</p>
    {#if detail}
      <p class="truncate text-white/80">{detail}</p>
    {/if}
    {#if hint}
      <p class="mt-0.5 text-xs text-white/55">{hint}</p>
    {/if}
    {#if actionLabel && onAction}
      <button
        type="button"
        class="mt-2 rounded-default bg-white/20 px-2 py-1 text-xs font-semibold hover:bg-white/30 transition-colors"
        onclick={() => {
          onAction?.();
          onDismiss();
        }}
      >
        {actionLabel}
      </button>
    {/if}
  </div>
  <button
    type="button"
    class="shrink-0 rounded-default p-0.5 text-white/60 transition-colors hover:bg-white/10 hover:text-white"
    aria-label={$t("mic_toast.dismiss")}
    onclick={onDismiss}
  >
    <X size={14} aria-hidden="true" />
  </button>
</div>
