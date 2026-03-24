<script lang="ts">
  import ConfirmAction from "../../../components/ui/ConfirmAction.svelte";
  import type { DictationEntry } from "../../../types";

  let {
    history,
    expandedEntryId,
    onToggleEntry,
    onDeleteEntry,
    onClearHistory,
  }: {
    history: DictationEntry[];
    expandedEntryId: string | null;
    onToggleEntry: (id: string) => void;
    onDeleteEntry: (id: string) => void | Promise<void>;
    onClearHistory: () => void | Promise<void>;
  } = $props();
</script>

<section class="surface-card">
  <div class="flex items-center justify-between gap-4 flex-wrap">
    <h3>History <span class="text-sm text-text-muted font-normal">({history.length})</span></h3>
    <ConfirmAction
      label="Clear all"
      confirmLabel="Yes, clear"
      confirmMessage="Clear all entries?"
      variant="danger"
      onConfirm={onClearHistory}
    />
  </div>

  <div class="flex flex-col gap-2 mt-2">
    {#each history as entry}
      {@const isExpanded = expandedEntryId === entry.id}
      <div class="rounded-default outline-1 outline-ghost-border bg-surface-1 overflow-hidden">
        <button
          onclick={() => onToggleEntry(entry.id)}
          class="w-full px-3 py-2.5 text-left cursor-pointer transition-colors duration-150 hover:bg-surface-3"
        >
          <span class="text-xs text-text-muted">{new Date(entry.timestamp).toLocaleString()}</span>
          <p class={`mt-1 mb-0 text-text-secondary text-sm leading-normal whitespace-pre-wrap break-words ${!isExpanded ? "line-clamp-5" : ""}`}>{entry.text}</p>
        </button>
        <div class="flex gap-1 px-3 pb-2">
          <button onclick={() => navigator.clipboard.writeText(entry.text)} class="btn btn-ghost btn-sm">Copy</button>
          <button onclick={() => onDeleteEntry(entry.id)} class="btn btn-ghost btn-sm text-danger">Delete</button>
        </div>
      </div>
    {/each}
  </div>
</section>
