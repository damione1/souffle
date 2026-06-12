<script lang="ts">
  import { t } from "svelte-i18n";

  let {
    notes,
    saveState,
    onNotesChange,
  }: {
    notes: string;
    saveState: "idle" | "pending" | "saved";
    onNotesChange: (value: string) => void;
  } = $props();
</script>

<section class="surface-card flex flex-col gap-3">
  <div class="flex items-center justify-between gap-2">
    <h3>{$t("meeting_notes.title")}</h3>
    {#if saveState === "saved"}
      <span class="text-xs text-text-muted">{$t("meeting_notes.saved")}</span>
    {:else if saveState === "pending"}
      <span class="text-xs text-text-muted">{$t("meeting_notes.saving")}</span>
    {/if}
  </div>
  <textarea
    value={notes}
    oninput={(event) => onNotesChange((event.target as HTMLTextAreaElement).value)}
    placeholder={$t("meeting_notes.placeholder")}
    class="field-input min-h-48 flex-1 resize-y font-normal leading-relaxed"
    aria-label={$t("meeting_notes.title")}
  ></textarea>
  <p class="text-xs text-text-muted">{$t("meeting_notes.hint")}</p>
</section>
