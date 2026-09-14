<script lang="ts">
  import { ChevronRight, FileText, Sparkles, Trash2, Users } from "@lucide/svelte";
  import { slide } from "svelte/transition";
  import { t } from "svelte-i18n";
  import CopyButton from "../../../components/ui/CopyButton.svelte";
  import { formatDuration } from "../../../utils";
  import type { TimelineItem } from "../controller.svelte";

  /** Icon colour per timeline item kind, keyed on the union so a third kind
   * has to pick a colour instead of inheriting the dictation one. */
  const KIND_ICON_CLASS: Record<TimelineItem["kind"], string> = {
    meeting: "text-accent",
    dictation: "text-text-muted",
  };

  let {
    item,
    expanded,
    isLive = false,
    onOpen,
    onRemove,
  }: {
    item: TimelineItem;
    expanded: boolean;
    isLive?: boolean;
    onOpen: () => void;
    onRemove: () => void;
  } = $props();

  const time = $derived(
    new Date(item.at).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" }),
  );

  let confirmingDelete = $state(false);
</script>

<div class="group">
  <div class="flex items-center gap-3 border-t border-rule py-3 transition-colors hover:bg-surface-2">
    <button onclick={onOpen} class="flex min-w-0 flex-1 cursor-pointer items-center gap-3 text-left">
      <span
        class="flex h-[22px] w-[22px] shrink-0 items-center justify-center {KIND_ICON_CLASS[item.kind]}"
        aria-hidden="true"
      >
        {#if item.kind === "meeting"}
          <Users size={16} />
        {:else}
          <FileText size={16} />
        {/if}
      </span>

      <span class="min-w-0 flex-1">
        <span class="block truncate text-[13.5px] text-text-secondary">{item.title}</span>
      </span>

      <span class="flex shrink-0 items-center gap-[11px] text-text-muted">
        {#if item.hasSummary}
          <span
            class={`flex ${item.summaryIsStale ? "text-warning" : "text-accent"}`}
            title={item.summaryIsStale ? $t("timeline.summary_stale") : $t("timeline.summary_ready")}
          >
            <Sparkles size={14} fill="currentColor" aria-hidden="true" />
          </span>
        {/if}
        {#if item.durationSeconds !== null}
          <span class="font-mono text-[11.5px]">{formatDuration(item.durationSeconds)}</span>
        {/if}
        <span class="font-mono text-[11.5px]">{time}</span>
        {#if item.kind === "meeting"}
          <ChevronRight size={15} aria-hidden="true" />
        {/if}
      </span>
    </button>

    <span class="flex shrink-0 items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100">
      {#if item.kind === "dictation"}
        <CopyButton text={item.title} />
      {/if}
      {#if !isLive}
        {#if confirmingDelete}
          <button
            onclick={() => { confirmingDelete = false; onRemove(); }}
            class="rounded-default px-1.5 py-0.5 text-xs text-danger-soft hover:bg-danger/15"
          >
            {$t("timeline.confirm_delete")}
          </button>
        {:else}
          <button
            onclick={() => (confirmingDelete = true)}
            onblur={() => (confirmingDelete = false)}
            class="cursor-pointer rounded-default p-1 text-text-muted hover:bg-surface-3 hover:text-danger-soft"
            aria-label={$t("timeline.delete")}
          >
            <Trash2 size={14} aria-hidden="true" />
          </button>
        {/if}
      {/if}
    </span>
  </div>

  {#if item.kind === "dictation" && expanded}
    <div transition:slide={{ duration: 150 }} class="pl-[34px] pb-3">
      <p class="whitespace-pre-wrap text-sm leading-relaxed text-text-secondary">{item.title}</p>
    </div>
  {/if}
</div>
