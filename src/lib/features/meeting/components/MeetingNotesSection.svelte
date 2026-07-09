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
    /** Hero variant: taller field, used as the lead of the meeting detail. */
    large?: boolean;
  } = $props();
</script>

<section class="surface-card flex flex-1 flex-col {large ? 'gap-[11px]' : 'gap-2.5 !px-5 !py-4'}">
  <div class="flex items-center justify-between gap-2">
    <h3 class="flex items-center gap-2 font-semibold {large ? 'text-sm text-text-primary' : 'text-[13px] text-text-tertiary'}">
      <PenLine size={large ? 15 : 14} class="text-accent" aria-hidden="true" />
      {$t("meeting_notes.title")}
    </h3>
    {#if saveState === "saved"}
      <span class="text-[11.5px] text-text-muted">{$t("meeting_notes.saved")}</span>
    {:else if saveState === "pending"}
      <span class="text-[11.5px] text-text-muted">{$t("meeting_notes.saving")}</span>
    {/if}
  </div>
  <textarea
    value={notes}
    oninput={(event) => onNotesChange((event.target as HTMLTextAreaElement).value)}
    placeholder={$t("meeting_notes.placeholder")}
    spellcheck="false"
    class={`w-full flex-1 resize-y rounded-[11px] border-none bg-input text-[13.5px] leading-[1.65] text-text-secondary outline-1 outline-ghost-border placeholder:text-text-muted focus:outline-accent/50 ${
      large ? "min-h-24 px-[15px] py-[13px]" : "min-h-[60px] px-3.5 py-[11px]"
    }`}
    aria-label={$t("meeting_notes.title")}
  ></textarea>
  {#if large}
    <p class="text-xs text-text-muted">{$t("meeting_notes.hint")}</p>
  {/if}
</section>
