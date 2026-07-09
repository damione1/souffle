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
    onAdd: (term: string, pronunciation: string | null, category: string | null) => void | Promise<void>;
    onDelete: (id: number) => void | Promise<void>;
  } = $props();

  let newTerm = $state("");
  let newPronunciation = $state("");
  let newCategory = $state("");
  let addError = $state("");

  async function handleAdd() {
    const term = newTerm.trim();
    if (!term) return;
    addError = "";
    try {
      await onAdd(term, newPronunciation.trim() || null, newCategory.trim() || null);
      newTerm = "";
      newPronunciation = "";
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

<section class="settings-group">
  <h3>{$t("settings_dictionary.title")}</h3>
  <div class="settings-rows">
  <div class="flex flex-col gap-3">
  <p class="setting-desc m-0">{$t("settings_dictionary.description")}</p>

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
      <label for="dict-pronunciation" class="field-label">{$t("settings_dictionary.pronunciation")}</label>
      <input
        id="dict-pronunciation"
        type="text"
        bind:value={newPronunciation}
        onkeydown={handleKeyDown}
        placeholder={$t("settings_dictionary.pronunciation_placeholder")}
        title={$t("settings_dictionary.pronunciation_desc")}
        class="field-input w-32"
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
    <p class="text-danger-soft text-xs">{addError}</p>
  {/if}
  </div>

  {#if entries.length > 0}
    <div class="flex flex-col gap-1">
      {#each entries as entry (entry.id)}
        <div class="flex items-center gap-2 rounded-[9px] bg-surface-2/60 px-2.5 py-1.5 text-sm text-text-secondary">
          <span class="flex-1 font-medium">
            {entry.term}{#if entry.pronunciation}<span class="text-text-muted font-normal"> · {entry.pronunciation}</span>{/if}
          </span>
          {#if entry.category}
            <span class="text-text-muted text-xs">{entry.category}</span>
          {/if}
          <button onclick={() => onDelete(entry.id)} class="btn btn-icon btn-ghost !min-h-0 !min-w-0 !p-1 text-text-muted hover:!text-danger-soft" aria-label={`${$t("settings_dictionary.delete")} ${entry.term}`}>
            <Trash2 size={14} />
          </button>
        </div>
      {/each}
    </div>
  {:else}
    <p class="text-text-muted text-xs italic">{$t("settings_dictionary.empty")}</p>
  {/if}
  </div>
</section>
