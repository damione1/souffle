<script lang="ts">
  import { Trash2, Plus } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import type { DictionaryEntry } from "../../../types";

  let {
    entries,
    onAdd,
    onDelete,
  }: {
    entries: DictionaryEntry[];
    onAdd: (term: string, phoneticCode: string | null, category: string | null) => void | Promise<void>;
    onDelete: (id: number) => void | Promise<void>;
  } = $props();

  let newTerm = $state("");
  let newCategory = $state("");
  let addError = $state("");

  async function handleAdd() {
    const term = newTerm.trim();
    if (!term) return;
    addError = "";
    try {
      await onAdd(term, null, newCategory.trim() || null);
      newTerm = "";
      newCategory = "";
    } catch (e) {
      addError = e instanceof Error ? e.message : String(e);
    }
  }

  function handleKeyDown(event: KeyboardEvent) {
    if (event.key === "Enter") {
      void handleAdd();
    }
  }
</script>

<section class="surface-card flex flex-col gap-3.5">
  <h3>{$t("settings_dictionary.title")}</h3>
  <p class="text-text-secondary text-sm">{$t("settings_dictionary.description")}</p>

  <div class="flex gap-1.5 items-end">
    <div class="flex flex-col gap-1 flex-1">
      <label for="dict-term" class="field-label">{$t("settings_dictionary.term")}</label>
      <input
        id="dict-term"
        type="text"
        bind:value={newTerm}
        onkeydown={handleKeyDown}
        placeholder={$t("settings_dictionary.term_placeholder")}
        class="field-input"
      />
    </div>
    <div class="flex flex-col gap-1">
      <label for="dict-category" class="field-label">{$t("settings_dictionary.category")}</label>
      <input
        id="dict-category"
        type="text"
        bind:value={newCategory}
        onkeydown={handleKeyDown}
        placeholder={$t("settings_dictionary.category_placeholder")}
        class="field-input w-28"
      />
    </div>
    <button onclick={handleAdd} class="btn btn-primary btn-icon" aria-label={$t("settings_dictionary.add_entry")} disabled={!newTerm.trim()}>
      <Plus size={16} />
    </button>
  </div>
  {#if addError}
    <p class="text-red-400 text-xs">{addError}</p>
  {/if}

  {#if entries.length > 0}
    <div class="flex flex-col gap-1 mt-1">
      {#each entries as entry (entry.id)}
        <div class="flex items-center gap-2 py-1.5 px-2 rounded-lg bg-surface-secondary/50 text-sm">
          <span class="flex-1 font-medium">{entry.term}</span>
          {#if entry.category}
            <span class="text-text-tertiary text-xs">{entry.category}</span>
          {/if}
          <button onclick={() => onDelete(entry.id)} class="btn btn-icon btn-ghost text-text-tertiary hover:text-red-400" aria-label={`${$t("settings_dictionary.delete")} ${entry.term}`}>
            <Trash2 size={14} />
          </button>
        </div>
      {/each}
    </div>
  {:else}
    <p class="text-text-tertiary text-xs italic">{$t("settings_dictionary.empty")}</p>
  {/if}
</section>
