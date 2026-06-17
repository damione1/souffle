<script lang="ts">
  import { PenLine } from "@lucide/svelte";
  import { t } from "svelte-i18n";

  let {
    notes,
    saveState,
    onNotesChange,
    large = false,
  }: {
    notes: string;
    saveState: "idle" | "pending" | "saved";
    onNotesChange: (value: string) => void;
    /** Hero variant: taller field, used as the focus of the meeting detail. */
    large?: boolean;
  } = $props();
</script>

<section class="surface-card flex flex-1 flex-col gap-3">
  <div class="flex items-center justify-between gap-2">
    <h3 class="flex items-center gap-2">
      <PenLine size={16} class="text-accent" aria-hidden="true" />
      {$t("meeting_notes.title")}
    </h3>
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
    class={`field-input flex-1 resize-y font-normal leading-relaxed ${large ? "min-h-[20rem]" : "min-h-48"}`}
    aria-label={$t("meeting_notes.title")}
  ></textarea>
  {#if large}
    <p class="text-xs text-text-muted">{$t("meeting_notes.hint")}</p>
  {/if}
</section>
