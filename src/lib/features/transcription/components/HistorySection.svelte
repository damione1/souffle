<script lang="ts">
  import { ChevronDown, Copy, Trash2 } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import ConfirmAction from "../../../components/ui/ConfirmAction.svelte";
  import type { DictationEntry, SearchResult } from "../../../types";
  import { findSnippet } from "../../../utils";

  let {
    history,
    filteredHistory,
    expandedEntryId,
    searchQuery = $bindable(),
    searchResults,
    onToggleEntry,
    onDeleteEntry,
    onClearHistory,
  }: {
    history: DictationEntry[];
    filteredHistory: DictationEntry[];
    expandedEntryId: string | null;
    searchQuery: string;
    searchResults: SearchResult[];
    onToggleEntry: (id: string) => void;
    onDeleteEntry: (id: string) => void | Promise<void>;
    onClearHistory: () => void | Promise<void>;
  } = $props();

  let hasSearchQuery = $derived(searchQuery.trim().length > 0);

  function collapsedPreview(text: string): string {
    return text.replace(/\s+/g, " ").trim();
  }
</script>

<section class="surface-card">
  <div class="flex items-center justify-between gap-4 flex-wrap">
    <h3>{$t("dictation_history.title")} <span class="text-sm text-text-muted font-normal">({history.length})</span></h3>
    <ConfirmAction
      label={$t("dictation_history.clear_all")}
      confirmLabel={$t("dictation_history.clear_confirm_label")}
      confirmMessage={$t("dictation_history.clear_confirm_msg")}
      variant="danger"
      onConfirm={onClearHistory}
    />
  </div>

  <input
    type="text"
    bind:value={searchQuery}
    placeholder={$t("dictation_history.search_placeholder")}
    class="field-input mt-2"
  />

  {#if hasSearchQuery && filteredHistory.length !== history.length}
    <p class="text-xs text-text-muted mt-1">
      {$t("dictation_history.search_results_count", { values: { count: filteredHistory.length, total: history.length } })}
    </p>
  {/if}

  <div class="flex flex-col gap-2 mt-2">
    {#if filteredHistory.length === 0 && hasSearchQuery}
      <p class="text-sm text-text-muted text-center py-4">{$t("dictation_history.no_search_results")}</p>
    {:else}
      {#each filteredHistory as entry}
        {@const isExpanded = expandedEntryId === entry.id}
        {@const preview = collapsedPreview(entry.text)}
        {@const snippet = hasSearchQuery ? findSnippet(searchResults, "dictation", entry.id) : null}
        <div class="rounded-default outline-1 outline-ghost-border bg-surface-1 overflow-hidden">
          <button
            onclick={() => onToggleEntry(entry.id)}
            class="w-full px-3 py-2.5 text-left cursor-pointer transition-colors duration-150 hover:bg-surface-3"
          >
            <span class="flex items-center justify-between gap-3">
              <span class="text-xs text-text-muted">{new Date(entry.timestamp).toLocaleString()}</span>
              <span class="inline-flex items-center gap-1 text-xs text-text-muted">
                {isExpanded ? $t("dictation_history.collapse") : $t("dictation_history.expand")}
                <ChevronDown size={14} class={`transition-transform duration-150 ${isExpanded ? "rotate-180" : ""}`} />
              </span>
            </span>
            {#if isExpanded}
              <p class="mt-1 mb-0 text-text-secondary text-sm leading-normal whitespace-pre-wrap break-words">{entry.text}</p>
            {:else if snippet}
              <p class="search-highlight mt-1 mb-0 text-text-secondary text-sm leading-normal break-words line-clamp-3">
                {@html snippet}
              </p>
            {:else}
              <p class="mt-1 mb-0 text-text-secondary text-sm leading-normal break-words line-clamp-3">{preview}</p>
            {/if}
          </button>
          {#if isExpanded}
            <div class="flex gap-1 px-3 pb-2">
              <button onclick={() => navigator.clipboard.writeText(entry.text)} class="btn btn-ghost btn-sm gap-1.5">
                <Copy size={14} />
                {$t("dictation_history.copy")}
              </button>
              <button onclick={() => onDeleteEntry(entry.id)} class="btn btn-ghost btn-sm gap-1.5 text-danger">
                <Trash2 size={14} />
                {$t("dictation_history.delete")}
              </button>
            </div>
          {/if}
        </div>
      {/each}
    {/if}
  </div>
</section>
