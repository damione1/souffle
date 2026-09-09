<script lang="ts">
  import { Trash2, Plus } from "@lucide/svelte";
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

  function handleKeyDown(event: KeyboardEvent) {
    if (event.key === "Enter" && !event.shiftKey) {
      void handleAdd();
    }
  }
</script>

<section class="settings-group">
  <h3>{$t("settings_snippets.title", { default: "Snippets vocaux" })}</h3>
  <div class="settings-rows">
    <div class="flex flex-col gap-3">
      <p class="setting-desc m-0">{$t("settings_snippets.description", { default: "Déclencheur parlé exact → expansion collée." })}</p>

      <div class="flex flex-col gap-2">
        <div class="flex flex-col gap-1">
          <label for="snippet-trigger" class="field-label">{$t("settings_snippets.trigger", { default: "Déclencheur" })}</label>
          <input
            id="snippet-trigger"
            type="text"
            bind:value={newTrigger}
            onkeydown={handleKeyDown}
            placeholder={$t("settings_snippets.trigger_placeholder", { default: "ex: signature mail" })}
            class="field-input"
          />
        </div>
        <div class="flex flex-col gap-1">
          <label for="snippet-expansion" class="field-label">{$t("settings_snippets.expansion", { default: "Expansion" })}</label>
          <textarea
            id="snippet-expansion"
            bind:value={newExpansion}
            placeholder={$t("settings_snippets.expansion_placeholder", { default: "Le texte collé..." })}
            class="field-input resize-y min-h-[60px]"
          ></textarea>
        </div>
        <div class="flex justify-end">
          <button onclick={handleAdd} class="btn btn-primary btn-sm" disabled={!newTrigger.trim() || !newExpansion.trim()}>
            <Plus size={16} class="mr-1.5" /> {$t("settings_snippets.add_entry", { default: "Ajouter" })}
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
            <div class="flex items-center gap-2">
              <span class="font-medium text-text">{entry.trigger}</span>
              <div class="flex-1"></div>
              <button onclick={() => onDelete(entry.id)} class="btn btn-icon btn-ghost !min-h-0 !min-w-0 !p-1 text-text-muted hover:!text-danger-soft" aria-label={`${$t("settings_snippets.delete", { default: "Supprimer" })} ${entry.trigger}`}>
                <Trash2 size={14} />
              </button>
            </div>
            <pre class="whitespace-pre-wrap font-sans text-xs text-text-muted m-0 max-h-[100px] overflow-y-auto">{entry.expansion}</pre>
          </div>
        {/each}
      </div>
      {#if updateError}
        <p class="text-danger-soft text-xs mt-2">{updateError}</p>
      {/if}
    {:else}
      <p class="text-text-muted text-xs italic">{$t("settings_snippets.empty", { default: "Aucun snippet." })}</p>
    {/if}
  </div>
</section>
