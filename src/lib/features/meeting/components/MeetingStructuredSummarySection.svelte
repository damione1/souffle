<script lang="ts">
  import { CircleHelp, ListChecks, Target } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import type { MeetingTranscript } from "../../../types";

  let {
    meeting,
    isRecordingMeeting,
    isSummarizing,
  }: {
    meeting: MeetingTranscript;
    isRecordingMeeting: boolean;
    isSummarizing: boolean;
  } = $props();

  let structured = $derived(meeting.structured_summary);

  let decisions = $derived(structured?.decisions ?? []);
  let actionItems = $derived(structured?.action_items ?? []);
  let openQuestions = $derived(structured?.open_questions ?? []);

  let hasContent = $derived(
    decisions.length > 0 || actionItems.length > 0 || openQuestions.length > 0,
  );

  let showStaleHint = $derived(
    hasContent && !isRecordingMeeting && !isSummarizing && meeting.summary_is_stale,
  );
</script>

{#if hasContent && !isSummarizing && !isRecordingMeeting}
  <section class="surface-card flex flex-col gap-3.5" style="animation: rise-in 220ms ease;">
    <div class="flex items-center justify-between gap-3 flex-wrap">
      <h3 class="flex items-center gap-2 text-sm font-semibold text-text-primary">
        <ListChecks size={15} class="text-accent" aria-hidden="true" />
        {$t("meeting_structured.title")}
      </h3>
      {#if showStaleHint}
        <span class="pill pill-warning">{$t("meeting_summary.summary_outdated")}</span>
      {/if}
    </div>

    {#if decisions.length > 0}
      <div class="flex flex-col gap-2">
        <h4 class="flex items-center gap-1.5 text-[12.5px] font-semibold text-text-tertiary">
          <Target size={13} class="text-accent" aria-hidden="true" />
          {$t("meeting_structured.decisions")}
        </h4>
        <div class="flex flex-wrap gap-2">
          {#each decisions as decision}
            <span class="rounded-full bg-accent/12 px-3 py-1.5 text-[12.5px] leading-snug text-text-secondary outline-1 outline-accent/20">
              {decision}
            </span>
          {/each}
        </div>
      </div>
    {/if}

    {#if actionItems.length > 0}
      <div class="flex flex-col gap-2">
        <h4 class="text-[12.5px] font-semibold text-text-tertiary">
          {$t("meeting_structured.action_items")}
        </h4>
        <div class="grid grid-cols-2 gap-2.5 max-[640px]:grid-cols-1">
          {#each actionItems as item}
            <div class="flex flex-col gap-1 rounded-xl bg-input px-3.5 py-[13px] outline-1 outline-ghost-border">
              <p class="m-0 text-[12.5px] leading-[1.55] text-text-secondary">{item.text}</p>
              {#if item.owner}
                <span class="font-mono text-[11px] text-text-muted">
                  {$t("meeting_structured.owner", { values: { owner: item.owner } })}
                </span>
              {/if}
            </div>
          {/each}
        </div>
      </div>
    {/if}

    {#if openQuestions.length > 0}
      <div class="flex flex-col gap-2">
        <h4 class="flex items-center gap-1.5 text-[12.5px] font-semibold text-text-tertiary">
          <CircleHelp size={13} class="text-accent" aria-hidden="true" />
          {$t("meeting_structured.open_questions")}
        </h4>
        <div class="flex flex-wrap gap-2">
          {#each openQuestions as question}
            <span class="rounded-full bg-input px-3 py-1.5 text-[12.5px] leading-snug text-text-secondary outline-1 outline-ghost-border">
              {question}
            </span>
          {/each}
        </div>
      </div>
    {/if}
  </section>
{/if}
