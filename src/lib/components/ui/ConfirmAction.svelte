<script lang="ts">
  import { Trash2 } from "@lucide/svelte";
  import { t } from "svelte-i18n";

  let {
    label,
    confirmLabel,
    confirmMessage,
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
    <span class="text-sm text-text-muted">{confirmMessage ?? $t("ui.are_you_sure")}</span>
    <button
      onclick={() => {
        onConfirm();
        confirming = false;
      }}
      class={`btn ${variant === "danger" ? "btn-danger" : ""}`}
    >
      {confirmLabel ?? $t("ui.yes")}
    </button>
    <button onclick={() => (confirming = false)} class="btn btn-ghost">{$t("ui.cancel")}</button>
  </div>
{:else}
  <button
    onclick={() => (confirming = true)}
    class={`btn btn-ghost gap-[7px] px-2.5 py-1.5 text-[12.5px] ${
      variant === "danger" ? "!text-danger-strong hover:!bg-danger/10" : ""
    }`}
  >
    {#if variant === "danger"}
      <Trash2 size={14} aria-hidden="true" />
    {/if}
    {label}
  </button>
{/if}
