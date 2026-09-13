<script lang="ts">
  import { AlarmClockOff, ClipboardCheck, MicOff, Square } from "@lucide/svelte";
  import { onDestroy, onMount } from "svelte";
  import { t } from "svelte-i18n";
  import Waveform from "../../components/Waveform.svelte";
  import Spinner from "../../components/ui/Spinner.svelte";
  import MeetingNotesSection from "../meeting/components/MeetingNotesSection.svelte";
  import TranscriptWordLine from "../meeting/components/TranscriptWordLine.svelte";
  import type { LiveParagraph } from "../meeting/live-transcript.svelte";
  import type { createMeetingController } from "../meeting/controller.svelte";
  import type { createTranscriptionController } from "../transcription/controller.svelte";
  import {
    elapsedSecondsSince,
    formatDuration,
    resolveSpeaker,
    segmentGap,
    speakerI18nKey,
    speakerTextClass,
  } from "../../utils";
  import {
    leadingRemovedCount,
    measureLeadingHeight,
    scrollTopAfterLeadingUnmount,
    windowedParagraphs,
  } from "./live-paragraph-window";
  import { liveSystemAudioNotice, liveSystemAudioState } from "../meeting/system-audio";

  let {
    mode,
    transcription,
    meeting,
  }: {
    mode: "dictation" | "meeting";
    transcription: ReturnType<typeof createTranscriptionController>;
    meeting: ReturnType<typeof createMeetingController>;
  } = $props();

  /** Auto-scroll only kicks in when already within this many px of the bottom. */
  const NEAR_BOTTOM_PX = 40;

  // Repaint driver only. The displayed value is always recomputed from the
  // wall clock below, so ticks dropped by WebKit's background throttling cost
  // nothing but refresh smoothness. A counter incremented per tick would
  // instead lose that time for good.
  let nowMs = $state(Date.now());
  let transcriptEl: HTMLDivElement | undefined = $state();
  let editingParagraphId = $state<number | null>(null);
  let editDraft = $state("");
  let editSaving = $state(false);

  const liveText = $derived(mode === "dictation" ? transcription.transcript : "");

  const liveParagraphs = $derived(
    mode === "meeting"
      ? windowedParagraphs(meeting.liveTranscript.committed, meeting.liveTranscript.tail)
      : [],
  );
  const liveTentativeDictation = $derived(
    mode === "dictation" ? transcription.tentative : "",
  );
  const liveTentativeMeeting = $derived(
    mode === "meeting" ? meeting.liveTranscript.tentative : [],
  );

  const lastIndexBySpeaker = $derived.by(() => {
    const map = new Map<string | null, number>();
    liveParagraphs.forEach((p, i) => map.set(p.speaker ?? null, i));
    return map;
  });

  const hasLiveContent = $derived(
    mode === "dictation"
      ? Boolean(liveText) || Boolean(liveTentativeDictation)
      : liveParagraphs.length > 0 || liveTentativeMeeting.length > 0,
  );


  const elapsed = $derived(
    formatDuration(elapsedSecondsSince(meeting.app.recordingStartedAtMs, nowMs)),
  );

  const stopping = $derived(
    mode === "dictation" ? transcription.isStopping : meeting.isStopping,
  );

  const systemAudioState = $derived(liveSystemAudioState(meeting.app.systemAudioStatus));
  const liveNotice = $derived(
    mode === "meeting" ? liveSystemAudioNotice(meeting.app.systemAudioStatus) : null,
  );

  const idleSilenceMinutes = $derived(
    meeting.idleSignal ? Math.max(1, Math.round(meeting.idleSignal.idle_seconds / 60)) : 0,
  );

  function stop() {
    if (stopping) return;
    if (mode === "dictation") {
      void transcription.toggleRecording();
    } else {
      void meeting.stopRecording();
    }
  }

  function paragraphEditable(paragraph: LiveParagraph, index: number): boolean {
    if (stopping || editSaving) return false;
    const isCommitted = meeting.liveTranscript.committed.some((item) => item.id === paragraph.id);
    if (!isCommitted) return false;
    const isLastForSpeaker = lastIndexBySpeaker.get(paragraph.speaker ?? null) === index;
    const hasTentative = isLastForSpeaker && liveTentativeMeeting.some(t => t.speaker === (paragraph.speaker ?? null));
    return !hasTentative;
  }

  function startParagraphEdit(paragraph: LiveParagraph) {
    editingParagraphId = paragraph.id;
    editDraft = paragraph.text;
  }

  function cancelParagraphEdit() {
    editingParagraphId = null;
    editDraft = "";
  }

  async function saveParagraphEdit() {
    if (editingParagraphId == null || editSaving) return;
    const trimmed = editDraft.trim();
    if (!trimmed) return;
    editSaving = true;
    try {
      await meeting.applyLiveParagraphEdit(editingParagraphId, trimmed);
      cancelParagraphEdit();
    } finally {
      editSaving = false;
    }
  }

  function handleParagraphKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      cancelParagraphEdit();
      return;
    }
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      void saveParagraphEdit();
    }
  }

  let isNearBottom = true;
  let scrollRafId: number | null = null;
  let pendingRemovedHeight = 0;
  let previousWindowIds: number[] = [];

  function handleScroll() {
    const el = transcriptEl;
    if (!el) return;
    isNearBottom = el.scrollHeight - el.scrollTop - el.clientHeight <= NEAR_BOTTOM_PX;
  }

  function scheduleAutoscroll() {
    if (!isNearBottom || scrollRafId !== null) return;
    scrollRafId = requestAnimationFrame(() => {
      scrollRafId = null;
      if (transcriptEl) transcriptEl.scrollTop = transcriptEl.scrollHeight;
    });
  }

  $effect.pre(() => {
    const nextIds = liveParagraphs.map((paragraph) => paragraph.id);
    const el = transcriptEl;
    if (el && !isNearBottom) {
      const removed = leadingRemovedCount(previousWindowIds, nextIds);
      if (removed > 0) {
        const gap = Number.parseFloat(getComputedStyle(el).rowGap || el.style.gap || "0") || 0;
        pendingRemovedHeight = measureLeadingHeight(el, removed, gap);
      }
    }
    previousWindowIds = nextIds;
  });

  $effect(() => {
    void liveText;
    void liveParagraphs;
    void liveTentativeDictation;
    void liveTentativeMeeting;
    const el = transcriptEl;
    if (el && pendingRemovedHeight > 0) {
      el.scrollTop = scrollTopAfterLeadingUnmount(el.scrollTop, pendingRemovedHeight);
      pendingRemovedHeight = 0;
    }
    scheduleAutoscroll();
  });

  onMount(() => {
    const tick = () => {
      nowMs = Date.now();
    };
    const timer = setInterval(tick, 1000);
    // WebKit pauses the interval while the window is occluded; the displayed
    // value is derived from the wall clock, so one catch-up on return is enough.
    const onVisibility = () => {
      if (document.visibilityState === "visible") tick();
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  });

  onDestroy(() => {
    if (scrollRafId !== null) cancelAnimationFrame(scrollRafId);
  });
</script>

<div class="flex flex-col gap-[18px]">
  <div class="flex items-center gap-3.5">
    <span
      class="inline-flex items-center gap-2 whitespace-nowrap bg-danger px-[11px] py-[5px] text-[12.5px] font-semibold text-on-danger"
    >
      <span class="h-2 w-2 rounded-full bg-on-danger" style="animation: pulse-soft 1.2s ease-in-out infinite;"></span>
      {mode === "meeting" ? $t("home.live_meeting") : $t("home.live_dictation")}
    </span>
    <div class="min-w-0 flex-1">
      <Waveform active variant="pill" />
    </div>
    <span class="shrink-0 font-mono text-sm text-text-tertiary">{elapsed}</span>
    <button
      onclick={stop}
      disabled={stopping}
      class="inline-flex shrink-0 cursor-pointer items-center gap-2 rounded-default bg-danger px-4 py-[9px] text-[13.5px] font-semibold text-on-danger transition-colors hover:bg-danger/90 disabled:cursor-default disabled:opacity-60"
    >
      {#if stopping}
        <Spinner />
        {$t("home.stopping")}
      {:else}
        <Square size={13} fill="currentColor" aria-hidden="true" />
        {$t("home.stop")}
      {/if}
    </button>
  </div>

  {#if mode === "dictation"}
    <div class="flex min-h-[340px] flex-col rounded-default px-[22px] py-5 outline-1 outline-border-soft">
      <p class="m-0 text-[19px] font-normal leading-[1.85] text-text-secondary">
        {liveText}{#if liveTentativeDictation}<span class="opacity-50">{segmentGap(liveText, liveTentativeDictation)}{liveTentativeDictation}</span>{/if}<span
          class="ml-0.5 inline-block h-5 w-0.5 bg-accent align-[-3px]"
          style="animation: blink 1s step-end infinite;"
        ></span>
      </p>
      <div class="flex-1"></div>
      {#if meeting.app.settings.auto_paste}
        <div class="mt-5 flex items-center gap-[9px] text-[12.5px] text-text-muted">
          <ClipboardCheck size={15} class="shrink-0 text-accent" aria-hidden="true" />
          {$t("home.autopaste_hint")}
        </div>
      {/if}
    </div>
  {:else}
    {#if liveNotice}
      <div
        class="flex items-start gap-3 rounded-default px-4 py-3 outline-1 outline-warning/30"
        title={liveNotice.detail ?? ""}
      >
        <MicOff size={16} class="mt-px shrink-0 text-warning" aria-hidden="true" />
        <p class="m-0 min-w-0 flex-1 text-sm text-text-secondary">
          <span class="font-semibold">{$t("meeting_header.system_audio_unavailable")}</span>
          {$t(liveNotice.key)}
        </p>
      </div>
    {/if}
    {#if mode === "meeting" && meeting.idleSignal}
      <div class="flex items-center gap-3 rounded-default px-4 py-3 outline-1 outline-warning/30">
        <AlarmClockOff size={16} class="shrink-0 text-warning" aria-hidden="true" />
        <p class="m-0 min-w-0 flex-1 text-sm text-warning">
          {$t("home.idle_silence_banner", { values: { minutes: idleSilenceMinutes } })}
        </p>
        <button onclick={stop} class="btn btn-danger btn-sm shrink-0" disabled={stopping}>
          {$t("home.idle_stop_now")}
        </button>
        <button onclick={() => meeting.dismissIdle()} class="btn btn-sm shrink-0">
          {$t("home.idle_keep_recording")}
        </button>
      </div>
    {/if}

    <div class="flex min-h-[300px] flex-col gap-4 rounded-default px-[18px] py-4 outline-1 outline-border-soft">
      <div class="flex items-center justify-between gap-3">
        <h3 class="text-[11px] font-semibold uppercase tracking-[0.1em] text-text-muted [font-variation-settings:'wdth'_88]">{$t("home.live_transcript")}</h3>
        <span class="inline-flex items-center gap-1.5 text-[11.5px] text-text-muted">
          <span
            class={`h-1.5 w-1.5 rounded-full ${systemAudioState === "active" ? "bg-accent" : "bg-surface-4"}`}
          ></span>
          {#if systemAudioState === "active"}
            {$t("home.system_audio_active")}
          {:else if systemAudioState === "pending"}
            {$t("home.system_audio_pending")}
          {:else}
            {$t("meeting_header.system_audio_unavailable")}
          {/if}
        </span>
      </div>
      <p class="m-0 text-[11.5px] text-text-muted">{$t("home.live_edit_hint")}</p>
      <div
        bind:this={transcriptEl}
        onscroll={handleScroll}
        class="flex max-h-[250px] flex-1 flex-col gap-4 overflow-y-auto pr-1.5"
      >
        {#if !hasLiveContent}
          <span class="text-sm text-text-muted">{$t("home.listening")}</span>
        {:else}
          {#each liveParagraphs as paragraph, i (paragraph.id)}
            {@const speaker = resolveSpeaker(paragraph.speaker)}
            {@const isLastForSpeaker = lastIndexBySpeaker.get(paragraph.speaker ?? null) === i}
            {@const speakerTentative = isLastForSpeaker ? liveTentativeMeeting.find(t => t.speaker === (paragraph.speaker ?? null))?.text : null}
            <div class="flex flex-col gap-[3px]" style="animation: rise-in 240ms ease;">
              <div class="flex items-center gap-2">
                {#if speaker}
                  <span class="text-[11.5px] font-semibold {speakerTextClass(speaker)}"
                  >{$t(speakerI18nKey(speaker))}</span>
                {/if}
                <span class="font-mono text-[10.5px] text-text-faint">{paragraph.timestamp}</span>
              </div>
              {#if editingParagraphId === paragraph.id}
                <div class="flex flex-col gap-2">
                  <textarea
                    bind:value={editDraft}
                    onkeydown={handleParagraphKeydown}
                    class="field-input min-h-[72px] resize-y text-[15px] leading-[1.75]"
                    disabled={editSaving}
                  ></textarea>
                  <div class="flex gap-2">
                    <button
                      onclick={() => void saveParagraphEdit()}
                      class="btn btn-sm"
                      disabled={editSaving || !editDraft.trim()}
                    >
                      {$t("home.live_edit_save")}
                    </button>
                    <button onclick={cancelParagraphEdit} class="btn btn-ghost btn-sm" disabled={editSaving}>
                      {$t("home.live_edit_cancel")}
                    </button>
                  </div>
                </div>
              {:else}
                <p
                  class="m-0 text-[15px] leading-[1.75] text-text-secondary"
                  class:cursor-text={paragraphEditable(paragraph, i)}
                  class:hover:outline-1={paragraphEditable(paragraph, i)}
                  class:hover:outline-ghost-border={paragraphEditable(paragraph, i)}
                  class:rounded-default={paragraphEditable(paragraph, i)}
                  class:px-1={paragraphEditable(paragraph, i)}
                  class:-mx-1={paragraphEditable(paragraph, i)}
                  ondblclick={() => {
                    if (paragraphEditable(paragraph, i)) startParagraphEdit(paragraph);
                  }}
                >
                  <TranscriptWordLine
                    text={paragraph.text}
                    onAddAlias={meeting.addDictionaryAlias}
                    class="m-0 inline text-[15px] leading-[1.75] text-text-secondary"
                  />
                  {#if speakerTentative}
                    <span class="opacity-50">{segmentGap(paragraph.text, speakerTentative)}{speakerTentative}</span>
                  {/if}
                </p>
              {/if}
            </div>
          {/each}
          {#each liveTentativeMeeting as tentative (tentative.speaker)}
            {#if !lastIndexBySpeaker.has(tentative.speaker)}
              {@const speaker = resolveSpeaker(tentative.speaker)}
              <div class="flex flex-col gap-[3px]" style="animation: rise-in 240ms ease;">
                <div class="flex items-center gap-2">
                  {#if speaker}
                    <span class="text-[11.5px] font-semibold {speakerTextClass(speaker)}"
                    >{$t(speakerI18nKey(speaker))}</span>
                  {/if}
                </div>
                <p class="m-0 text-[15px] leading-[1.75] text-text-secondary opacity-50">{tentative.text}</p>
              </div>
            {/if}
          {/each}
        {/if}
      </div>
    </div>

    <MeetingNotesSection
      notes={meeting.notesDraft}
      saveState={meeting.notesSaveState}
      onNotesChange={meeting.onNotesChange}
    />
  {/if}
</div>
