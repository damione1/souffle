<script lang="ts">
  import { Clock3, Sparkles, WandSparkles } from "@lucide/svelte";
  import { t } from "svelte-i18n";
  import CopyButton from "../../../components/ui/CopyButton.svelte";
  import Spinner from "../../../components/ui/Spinner.svelte";
  import StatusBanner from "../../../components/ui/StatusBanner.svelte";
  import type { MeetingTranscript, SummarizeProgress, SummaryModelDescriptor, SummaryTemplate, TranscriptionSegment } from "../../../types";

  const builtInTemplateNameKeys: Record<string, string> = {
    default: "summary_templates.template_default",
    detailed_minutes: "summary_templates.template_detailed_minutes",
    brief_overview: "summary_templates.template_brief_overview",
  };

  let {
    meeting,
    isRecordingMeeting,
    segments,
    summaryAvailable,
    summaryModels,
    selectedModel,
    onSelectModel,
    summaryTemplates,
    selectedTemplateId,
    onSelectTemplate,
    onSummarize,
    isSummarizing,
    summaryStream,
    summaryStage,
    summaryStageCurrent,
    summaryStageTotal,
  }: {
    meeting: MeetingTranscript;
    isRecordingMeeting: boolean;
    segments: TranscriptionSegment[];
    summaryAvailable: boolean;
    summaryModels: SummaryModelDescriptor[];
    selectedModel: string;
    onSelectModel: (modelId: string) => void;
    summaryTemplates: SummaryTemplate[];
    selectedTemplateId: string;
    onSelectTemplate: (templateId: string) => void;
    onSummarize: () => void | Promise<void>;
    isSummarizing: boolean;
    summaryStream: string;
    summaryStage: SummarizeProgress["stage"] | null;
    summaryStageCurrent: number | null;
    summaryStageTotal: number | null;
  } = $props();

  function templateName(template: SummaryTemplate): string {
    const key = builtInTemplateNameKeys[template.id];
    return key ? $t(key) : template.name;
  }

  let stageLabel = $derived.by(() => {
    if (!summaryStage) return "";
    if (summaryStage === "map" && summaryStageCurrent && summaryStageTotal) {
      return $t("meeting_summary.stage_map", { values: { current: summaryStageCurrent, total: summaryStageTotal } });
    }
    if (summaryStage === "combine") {
      if (summaryStageCurrent && summaryStageTotal) {
        return $t("meeting_summary.stage_combine_progress", { values: { current: summaryStageCurrent, total: summaryStageTotal } });
      }
      return $t("meeting_summary.stage_combine");
    }
    if (summaryStage === "extract") {
      return $t("meeting_summary.stage_extract");
    }
    return "";
  });

  let keyPoints = $derived.by(() => {
    const summary = meeting.summary?.trim();
    if (!summary) return [];

    return summary
      .split("\n")
      .filter((line) => line.trim())
      .filter((line) => /^[-•*]\s/.test(line.trim()) || /^\d+[.)]\s/.test(line.trim()))
      .slice(0, 4);
  });

  let generatedWithLabel = $derived(
    summaryModels.find((model) => model.id === meeting.summary_model)?.label
    ?? meeting.summary_model
    ?? "",
  );

  type SummaryPhase =
    | "summary_recording"
    | "summary_stale"
    | "summary_current"
    | "recording"
    | "empty";

  let summaryPhase = $derived.by((): SummaryPhase => {
    if (meeting.summary) {
      if (isRecordingMeeting) return "summary_recording";
      if (meeting.summary_is_stale) return "summary_stale";
      return "summary_current";
    }
    if (isRecordingMeeting) return "recording";
    return "empty";
  });

  const canGenerate = $derived(
    !isRecordingMeeting && segments.length > 0 && summaryAvailable && summaryModels.length > 0,
  );
</script>

{#if summaryPhase === "empty" && canGenerate && !isSummarizing}
  <!-- Collapsed CTA until a summary exists, per the design. -->
  <button
    onclick={onSummarize}
    class="surface-card flex cursor-pointer items-center gap-[13px] !px-[18px] !py-[15px] text-left text-text-primary transition-[outline-color,transform] duration-150 hover:outline-accent/40 active:scale-[0.99]"
  >
    <span class="flex h-9 w-9 shrink-0 items-center justify-center rounded-[10px] bg-accent/12 text-accent">
      <WandSparkles size={17} aria-hidden="true" />
    </span>
    <span class="flex flex-1 flex-col gap-0.5">
      <span class="text-sm font-semibold">{$t("meeting_summary.generate_summary")}</span>
      <span class="text-xs text-text-muted">{$t("meeting_summary.generate_hint")}</span>
    </span>
    <span class="shrink-0 rounded-[9px] bg-accent/12 px-3.5 py-[7px] text-[12.5px] font-semibold text-accent">
      {$t("meeting_summary.generate_cta")}
    </span>
  </button>
{:else}
  <section class="surface-card flex flex-col gap-3.5" style="animation: rise-in 220ms ease;">
    <div class="flex items-center justify-between gap-4 flex-wrap">
      <h3 class="flex items-center gap-2 text-sm font-semibold text-text-primary">
        <Sparkles size={15} fill="currentColor" class="text-accent" aria-hidden="true" />
        {$t("meeting_summary.title")}
      </h3>
      <div class="flex items-center gap-2.5">
        {#if generatedWithLabel}
          <span class="font-mono text-[11px] text-text-muted">
            {generatedWithLabel} · {$t("meeting_summary.local_badge")}
          </span>
        {/if}
        {#if meeting.summary}
          <CopyButton text={meeting.summary} />
        {/if}
      </div>
    </div>

    {#if keyPoints.length > 0}
      <div class="grid grid-cols-2 gap-2.5 max-[640px]:grid-cols-1">
        {#each keyPoints as point, index}
          <div class="flex items-start gap-[11px] rounded-xl bg-input px-3.5 py-[13px] outline-1 outline-ghost-border">
            <span class="flex h-[22px] w-[22px] shrink-0 items-center justify-center rounded-[7px] bg-accent/14 font-mono text-[11px] font-medium text-accent">{index + 1}</span>
            <p class="m-0 text-[12.5px] leading-[1.55] text-text-tertiary">{point.replace(/^[-•*\d.)]+\s*/, "").trim()}</p>
          </div>
        {/each}
      </div>
    {/if}

    {#if summaryPhase === "summary_recording"}
      <div class="flex items-center gap-2 flex-wrap">
        <span class="pill pill-warning">{$t("meeting_summary.recording_in_progress")}</span>
        <span class="text-sm text-text-muted">{$t("meeting_summary.can_regenerate")}</span>
      </div>
    {:else if summaryPhase === "summary_stale"}
      <div class="flex items-center gap-2 flex-wrap">
        <span class="pill pill-warning">{$t("meeting_summary.summary_outdated")}</span>
        <span class="text-sm text-text-muted">{$t("meeting_summary.new_audio_added")}</span>
      </div>
    {/if}

    {#if summaryPhase === "summary_recording" || summaryPhase === "summary_stale" || summaryPhase === "summary_current"}
      <div class="min-h-[100px] max-h-[360px] overflow-y-auto whitespace-pre-wrap rounded-xl bg-input p-3.5 text-sm leading-relaxed text-text-secondary outline-1 outline-ghost-border">{meeting.summary}</div>
    {:else if summaryPhase === "recording"}
      <div class="flex items-center gap-2.5 py-2 text-sm text-text-muted">
        <Clock3 size={16} aria-hidden="true" />
        {$t("meeting_summary.stop_to_generate")}
      </div>
    {/if}

    {#if isSummarizing}
      {#if stageLabel}
        <div class="flex items-center gap-2 text-xs text-text-muted">
          <Spinner />
          {stageLabel}
        </div>
      {/if}
      <div class="min-h-[80px] overflow-y-auto whitespace-pre-wrap rounded-xl bg-input p-3.5 text-sm leading-relaxed text-text-secondary outline-1 outline-ghost-border">{summaryStream}<span class="text-accent" style="animation: blink 1s step-end infinite;">|</span></div>
    {/if}

    {#if !isRecordingMeeting && segments.length > 0}
      {#if summaryAvailable && summaryModels.length > 0}
        <div class="flex gap-2 items-center flex-wrap">
          <select
            value={selectedModel}
            disabled={isSummarizing}
            onchange={(event) => onSelectModel((event.currentTarget as HTMLSelectElement).value)}
            class="field-select"
          >
            {#each summaryModels as model}
              <option value={model.id}>{model.label}</option>
            {/each}
          </select>
          {#if summaryTemplates.length > 0}
            <select
              value={selectedTemplateId}
              disabled={isSummarizing}
              onchange={(event) => onSelectTemplate((event.currentTarget as HTMLSelectElement).value)}
              class="field-select"
              aria-label={$t("meeting_summary.template")}
            >
              {#each summaryTemplates as template (template.id)}
                <option value={template.id}>{templateName(template)}</option>
              {/each}
            </select>
          {/if}
          <button onclick={onSummarize} disabled={isSummarizing} class="btn btn-primary whitespace-nowrap">
            {#if isSummarizing}
              <Spinner />
              {$t("meeting_summary.generating")}
            {:else}
              {meeting.summary ? $t("meeting_summary.regenerate_summary") : $t("meeting_summary.generate_summary")}
            {/if}
          </button>
        </div>
      {:else}
        <div class="flex items-center gap-2 py-2">
          <span class="status-dot"></span>
          <span class="text-sm text-text-muted">{$t("meeting_summary.no_summary_provider")}</span>
        </div>
      {/if}
    {/if}
  </section>
{/if}
