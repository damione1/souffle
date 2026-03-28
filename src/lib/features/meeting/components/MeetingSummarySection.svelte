<script lang="ts">
  import { Clock3, WandSparkles } from "@lucide/svelte";
  import CopyButton from "../../../components/ui/CopyButton.svelte";
  import EmptyState from "../../../components/ui/EmptyState.svelte";
  import Spinner from "../../../components/ui/Spinner.svelte";
  import StatusBanner from "../../../components/ui/StatusBanner.svelte";
  import type { MeetingTranscript, OllamaModelDescriptor, TranscriptionSegment } from "../../../types";

  let {
    meeting,
    isRecordingMeeting,
    segments,
    ollamaAvailable,
    summaryModels,
    selectedModel,
    onSelectModel,
    onSummarize,
    isSummarizing,
    summaryStream,
  }: {
    meeting: MeetingTranscript;
    isRecordingMeeting: boolean;
    segments: TranscriptionSegment[];
    ollamaAvailable: boolean;
    summaryModels: OllamaModelDescriptor[];
    selectedModel: string;
    onSelectModel: (modelId: string) => void;
    onSummarize: () => void | Promise<void>;
    isSummarizing: boolean;
    summaryStream: string;
  } = $props();

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
    | "summary_recording"   // has summary + currently recording
    | "summary_stale"       // has summary but outdated
    | "summary_current"     // has fresh summary
    | "recording"           // recording, no summary yet
    | "empty";              // stopped, no summary

  let summaryPhase = $derived.by((): SummaryPhase => {
    if (meeting.summary) {
      if (isRecordingMeeting) return "summary_recording";
      if (meeting.summary_is_stale) return "summary_stale";
      return "summary_current";
    }
    if (isRecordingMeeting) return "recording";
    return "empty";
  });
</script>

<section class="surface-card flex flex-col gap-3">
  <div class="flex items-center justify-between gap-4 flex-wrap">
    <h3>Summary</h3>
    {#if meeting.summary}
      <CopyButton text={meeting.summary} />
    {/if}
  </div>

  {#if keyPoints.length > 0}
    <div class="grid grid-cols-[repeat(auto-fill,minmax(200px,1fr))] gap-3">
      {#each keyPoints as point, index}
        <div class="flex gap-3 p-3.5 bg-surface-2 rounded-default outline-1 outline-ghost-border items-start">
          <span class="flex items-center justify-center w-6 h-6 rounded-full bg-accent-blue/15 text-accent-blue text-xs font-bold shrink-0">{index + 1}</span>
          <p class="m-0 text-sm text-text-secondary leading-normal">{point.replace(/^[-•*\d.)]+\s*/, "").trim()}</p>
        </div>
      {/each}
    </div>
  {/if}

  {#if summaryPhase === "summary_recording"}
    <div class="flex items-center gap-2 flex-wrap">
      <span class="pill pill-warning">Recording in progress</span>
      <span class="text-sm text-text-muted">You can regenerate the summary once the recording is stopped.</span>
    </div>
  {:else if summaryPhase === "summary_stale"}
    <div class="flex items-center gap-2 flex-wrap">
      <span class="pill pill-warning">Summary outdated</span>
      <span class="text-sm text-text-muted">New audio was added since this summary was generated.</span>
    </div>
  {/if}

  {#if summaryPhase === "summary_recording" || summaryPhase === "summary_stale" || summaryPhase === "summary_current"}
    <div class="p-3 bg-surface-1 rounded-default outline-1 outline-ghost-border text-text-secondary whitespace-pre-wrap min-h-[100px] max-h-[360px] overflow-y-auto text-sm leading-relaxed">{meeting.summary}</div>
    {#if generatedWithLabel}
      <span class="pill pill-muted self-start">Generated with {generatedWithLabel}</span>
    {/if}
  {:else if summaryPhase === "recording"}
    <EmptyState message="Stop the recording to generate a summary.">
      {#snippet icon()}
        <Clock3 size={32} strokeWidth={1.5} />
      {/snippet}
    </EmptyState>
  {:else}
    <EmptyState message="No summary yet. Generate one below.">
      {#snippet icon()}
        <WandSparkles size={32} strokeWidth={1.5} />
      {/snippet}
    </EmptyState>
  {/if}

  {#if isSummarizing}
    <div class="p-3 bg-surface-1 rounded-default outline-1 outline-ghost-border text-text-secondary whitespace-pre-wrap min-h-[80px] overflow-y-auto text-sm leading-relaxed">{summaryStream}<span class="text-text-muted animate-pulse">|</span></div>
  {/if}

  {#if !isRecordingMeeting && segments.length > 0}
    {#if ollamaAvailable && summaryModels.length > 0}
      <div class="flex gap-2 items-center">
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
        <button onclick={onSummarize} disabled={isSummarizing} class="btn btn-primary">
          {#if isSummarizing}
            <Spinner />
            Generating...
          {:else}
            {meeting.summary ? "Re-generate Summary" : "Generate Summary"}
          {/if}
        </button>
      </div>
    {:else if !ollamaAvailable}
      <div class="flex items-center gap-2 py-2">
        <span class="status-dot"></span>
        <span class="text-sm text-text-muted">Connect Ollama in Settings to enable summaries.</span>
      </div>
    {:else}
      <StatusBanner message="No compatible Ollama model found. Install a model like Llama or Mistral to enable summaries." />
    {/if}
  {/if}
</section>
