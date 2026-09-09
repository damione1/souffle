<script lang="ts">
  import { Pencil, Plus, Trash2 } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import type { SnippetEntry } from "../../../types";

  let {
    entries,
    onAdd,
    onDelete,
    onUpdate,
  }: {
    entries: SnippetEntry[];
    onAdd: (trigger: string, expansion: string) => void | Promise<void>;
    onDelete: (id: number) => void | Promise<void>;
    onUpdate: (id: number, trigger: string, expansion: string) => void | Promise<void>;
  } = $props();

  let newTrigger = $state("");
  let newExpansion = $state("");
  let addError = $state("");

  // Inline edit of one entry at a time; the row swaps to a form.
  let editingId = $state<number | null>(null);
  let editTrigger = $state("");
  let editExpansion = $state("");
  let updateError = $state("");

  async function handleAdd() {
    const trigger = newTrigger.trim();
    const expansion = newExpansion.trim();
    if (!trigger || !expansion) return;
    addError = "";
    try {
      await onAdd(trigger, expansion);
      newTrigger = "";
      newExpansion = "";
    } catch (e) {
      addError = e instanceof Error ? e.message : String(e);
    }
  }

  function startEdit(entry: SnippetEntry) {
    editingId = entry.id;
    editTrigger = entry.trigger;
    editExpansion = entry.expansion;
    updateError = "";
  }

  function cancelEdit() {
    editingId = null;
    updateError = "";
  }

  async function handleUpdate() {
    if (editingId === null) return;
    const trigger = editTrigger.trim();
    const expansion = editExpansion.trim();
    if (!trigger || !expansion) return;
    updateError = "";
    try {
      await onUpdate(editingId, trigger, expansion);
      editingId = null;
    } catch (e) {
      updateError = e instanceof Error ? e.message : String(e);
    }
  }

  function handleAddKeyDown(event: KeyboardEvent) {
    if (event.key === "Enter" && !event.shiftKey) {
      void handleAdd();
    }
  }

  function handleEditKeyDown(event: KeyboardEvent) {
    if (event.key === "Enter" && !event.shiftKey) {
      void handleUpdate();
    } else if (event.key === "Escape") {
      cancelEdit();
    }
  }
</script>

<section class="settings-group">
  <h3>{$t("settings_snippets.title")}</h3>
  <div class="settings-rows">
    <div class="flex flex-col gap-3">
      <p class="setting-desc m-0">{$t("settings_snippets.description")}</p>

      <div class="flex flex-col gap-2">
        <div class="flex flex-col gap-1">
          <label for="snippet-trigger" class="field-label">{$t("settings_snippets.trigger")}</label>
          <input
            id="snippet-trigger"
            type="text"
            bind:value={newTrigger}
            onkeydown={handleAddKeyDown}
            placeholder={$t("settings_snippets.trigger_placeholder")}
            class="field-input"
          />
        </div>
        <div class="flex flex-col gap-1">
          <label for="snippet-expansion" class="field-label">{$t("settings_snippets.expansion")}</label>
          <textarea
            id="snippet-expansion"
            bind:value={newExpansion}
            placeholder={$t("settings_snippets.expansion_placeholder")}
            class="field-input resize-y min-h-[60px]"
          ></textarea>
        </div>
        <div class="flex justify-end">
          <button onclick={handleAdd} class="btn btn-primary btn-sm" disabled={!newTrigger.trim() || !newExpansion.trim()}>
            <Plus size={16} class="mr-1.5" /> {$t("settings_snippets.add_entry")}
          </button>
        </div>
      </div>
      {#if addError}
        <p class="text-danger-soft text-xs">{addError}</p>
      {/if}
    </div>

    {#if entries.length > 0}
      <div class="flex flex-col gap-1 mt-2">
        {#each entries as entry (entry.id)}
          <div class="flex flex-col gap-2 rounded-[9px] bg-surface-2/60 px-2.5 py-1.5 text-sm text-text-secondary">
            {#if editingId === entry.id}
              <div class="flex flex-col gap-1">
                <label for={`snippet-edit-trigger-${entry.id}`} class="field-label">{$t("settings_snippets.trigger")}</label>
                <input
                  id={`snippet-edit-trigger-${entry.id}`}
                  type="text"
                  bind:value={editTrigger}
                  onkeydown={handleEditKeyDown}
                  class="field-input"
                />
              </div>
              <div class="flex flex-col gap-1">
                <label for={`snippet-edit-expansion-${entry.id}`} class="field-label">{$t("settings_snippets.expansion")}</label>
                <textarea
                  id={`snippet-edit-expansion-${entry.id}`}
                  bind:value={editExpansion}
                  class="field-input resize-y min-h-[60px]"
                ></textarea>
              </div>
              <div class="flex justify-end gap-1.5">
                <button onclick={cancelEdit} class="btn btn-ghost btn-sm">
                  {$t("settings_snippets.cancel")}
                </button>
                <button onclick={handleUpdate} class="btn btn-primary btn-sm" disabled={!editTrigger.trim() || !editExpansion.trim()}>
                  {$t("settings_snippets.save")}
                </button>
              </div>
              {#if updateError}
                <p class="text-danger-soft text-xs m-0">{updateError}</p>
              {/if}
            {:else}
              <div class="flex items-center gap-2">
                <span class="font-medium text-text">{entry.trigger}</span>
                <div class="flex-1"></div>
                <button onclick={() => startEdit(entry)} class="btn btn-icon btn-ghost !min-h-0 !min-w-0 !p-1 text-text-muted" aria-label={`${$t("settings_snippets.edit")} ${entry.trigger}`}>
                  <Pencil size={14} />
                </button>
                <button onclick={() => onDelete(entry.id)} class="btn btn-icon btn-ghost !min-h-0 !min-w-0 !p-1 text-text-muted hover:!text-danger-soft" aria-label={`${$t("settings_snippets.delete")} ${entry.trigger}`}>
                  <Trash2 size={14} />
                </button>
              </div>
              <pre class="whitespace-pre-wrap font-sans text-xs text-text-muted m-0 max-h-[100px] overflow-y-auto">{entry.expansion}</pre>
            {/if}
          </div>
        {/each}
      </div>
    {:else}
      <p class="text-text-muted text-xs italic">{$t("settings_snippets.empty")}</p>
    {/if}
  </div>
</section>
