<script lang="ts">
  import { t } from "svelte-i18n";

  let {
    heardAs,
    onClose,
    onSave,
  }: {
    heardAs: string;
    onClose: () => void;
    onSave: (term: string, pronunciation: string | null) => void | Promise<void>;
  } = $props();

  let termDraft = $state("");
  let pronunciationDraft = $state("");
  let isSaving = $state(false);
  let saveError = $state("");

  $effect(() => {
    termDraft = "";
    pronunciationDraft = heardAs;
    saveError = "";
  });

  async function save() {
    const term = termDraft.trim();
    if (!term) return;
    const pronunciation = pronunciationDraft.trim() || null;
    isSaving = true;
    saveError = "";
    try {
      await onSave(term, pronunciation);
      onClose();
    } catch (e) {
      saveError = e instanceof Error ? e.message : String(e);
    } finally {
      isSaving = false;
    }
  }
</script>

<button
  type="button"
  class="fixed inset-0 z-10 cursor-default"
  aria-label={$t("ui.cancel")}
  onclick={onClose}
></button>

<div class="absolute left-0 top-full z-20 mt-1.5 w-72 rounded-[11px] bg-surface-1 p-3 shadow-lg outline-1 outline-ghost-border">
  <p class="m-0 mb-2 text-[11px] font-semibold uppercase tracking-[0.12em] text-text-muted">
    {$t("dictionary_alias.title")}
  </p>
  <p class="m-0 mb-3 text-[12px] leading-relaxed text-text-muted">
    {$t("dictionary_alias.description")}
  </p>

  <div class="mb-2 flex flex-col gap-1">
    <label for="dict-alias-term" class="field-label">{$t("dictionary_alias.term")}</label>
    <input
      id="dict-alias-term"
      bind:value={termDraft}
      class="field-input text-[13px]"
      placeholder={$t("dictionary_alias.term_placeholder")}
      onkeydown={(e) => {
        if (e.key === "Enter") void save();
        if (e.key === "Escape") onClose();
      }}
    />
  </div>

  <div class="mb-3 flex flex-col gap-1">
    <label for="dict-alias-pronunciation" class="field-label">{$t("dictionary_alias.pronunciation")}</label>
    <input
      id="dict-alias-pronunciation"
      bind:value={pronunciationDraft}
      class="field-input text-[13px]"
      placeholder={$t("dictionary_alias.pronunciation_placeholder")}
      title={$t("dictionary_alias.pronunciation_desc")}
      onkeydown={(e) => {
        if (e.key === "Enter") void save();
        if (e.key === "Escape") onClose();
      }}
    />
  </div>

  {#if saveError}
    <p class="mb-2 text-xs text-danger-soft">{saveError}</p>
  {/if}

  <button
    class="btn btn-primary w-full text-[12.5px]"
    disabled={isSaving || !termDraft.trim()}
    onclick={() => void save()}
  >
    {$t("dictionary_alias.save")}
  </button>
</div>
