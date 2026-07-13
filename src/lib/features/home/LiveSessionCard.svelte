<script lang="ts">
  import { AlarmClockOff, ClipboardCheck, Square } from "@lucide/svelte";
  import { onDestroy, onMount } from "svelte";
  import { t } from "svelte-i18n";
  import Waveform from "../../components/Waveform.svelte";
  import Spinner from "../../components/ui/Spinner.svelte";
  import MeetingNotesSection from "../meeting/components/MeetingNotesSection.svelte";
  import type { createMeetingController } from "../meeting/controller.svelte";
  import type { createTranscriptionController } from "../transcription/controller.svelte";
  import { resolveSpeakerLabel } from "../../utils";

  let {
    mode,
    transcription,
    meeting,
  }: {
    mode: "dictation" | "meeting";
    transcription: ReturnType<typeof createTranscriptionController>;
    meeting: ReturnType<typeof createMeetingController>;
  } = $props();

  // Live paragraphs only ever tag Me/Them today (persistent speaker
  // identities aren't wired into the live pipeline yet), but resolve
  // through the same helper as the finished transcript view for consistency
  // and forward-compatibility.
  const liveSpeakers = $derived(meeting.meeting?.speakers ?? []);

  /** Only the most recent paragraphs stay in the DOM; older ones are still in
   * `meeting.liveTranscript.committed` but are never rendered live. */
  const LIVE_PARAGRAPH_WINDOW = 30;
  /** Auto-scroll only kicks in when already within this many px of the bottom. */
  const NEAR_BOTTOM_PX = 40;

  let elapsedSeconds = $state(0);
  let transcriptEl: HTMLDivElement | undefined = $state();

  const liveText = $derived(mode === "dictation" ? transcription.transcript : "");

  // Meetings render through the same paragraph grouping as the finished
  // transcript, so diarized sessions show Me/Them live: segments are ordered
  // by time and split on speaker changes instead of one undifferentiated blob.
  // `committed` only grows by push and can hold hours of history, so slice it
  // to the window first: slicing the last N elements is O(N), not O(meeting
  // length), which keeps this derived cheap on every incoming segment.
  const liveParagraphs = $derived(
    mode === "meeting"
      ? [...meeting.liveTranscript.committed.slice(-LIVE_PARAGRAPH_WINDOW), ...meeting.liveTranscript.tail]
        .slice(-LIVE_PARAGRAPH_WINDOW)
      : [],
  );
  const liveTentative = $derived(mode === "meeting" ? meeting.liveTranscript.tentative : "");

  const hasLiveContent = $derived(
    mode === "dictation" ? Boolean(liveText) : liveParagraphs.length > 0 || Boolean(liveTentative),
  );

  const elapsed = $derived(
    `${Math.floor(elapsedSeconds / 60)}:${`${elapsedSeconds % 60}`.padStart(2, "0")}`,
  );

  const stopping = $derived(
    mode === "dictation" ? transcription.isStopping : meeting.isStopping,
  );

  const systemAudioActive = $derived(Boolean(meeting.app.systemAudioStatus?.active));

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

  // Track whether the user is (still) parked near the bottom, independent of
  // content changes, so a burst of new segments never fights a manual scroll
  // up to read earlier text.
  let isNearBottom = true;
  let scrollRafId: number | null = null;

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

  // Keep the latest words in view as they stream in, coalesced to one scroll
  // per frame instead of one per segment.
  $effect(() => {
    void liveText;
    void liveParagraphs;
    void liveTentative;
    scheduleAutoscroll();
  });

  onMount(() => {
    const timer = setInterval(() => {
      elapsedSeconds += 1;
    }, 1000);
    return () => clearInterval(timer);
  });

  onDestroy(() => {
    if (scrollRafId !== null) cancelAnimationFrame(scrollRafId);
  });
</script>

<div class="flex flex-col gap-[18px]">
  <div class="flex items-center gap-3.5">
    <span
      class="inline-flex items-center gap-2 whitespace-nowrap rounded-full bg-danger/13 px-[13px] py-1.5 text-[12.5px] font-semibold text-danger-soft outline-1 outline-danger/28"
    >
      <span class="h-2 w-2 rounded-full bg-danger" style="animation: pulse-soft 1.2s ease-in-out infinite;"></span>
      {mode === "meeting" ? $t("home.live_meeting") : $t("home.live_dictation")}
    </span>
    <div class="min-w-0 flex-1">
      <Waveform active variant="pill" />
    </div>
    <span class="shrink-0 font-mono text-sm text-text-tertiary">{elapsed}</span>
    <button
      onclick={stop}
      disabled={stopping}
      class="inline-flex shrink-0 cursor-pointer items-center gap-2 rounded-[11px] bg-danger px-4 py-[9px] text-[13.5px] font-semibold text-on-danger transition-colors hover:bg-danger/90 disabled:cursor-default disabled:opacity-60"
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
    <!-- Dictation hero: the words are the whole surface. -->
    <div class="flex min-h-[340px] flex-col rounded-[18px] bg-surface-1 p-[30px] px-8 outline-1 outline-ghost-border">
      <p class="m-0 text-[19px] font-normal leading-[1.85] text-text-secondary">
        {liveText}<span
          class="ml-0.5 inline-block h-5 w-0.5 bg-accent align-[-3px]"
          style="animation: blink 1s step-end infinite;"
        ></span>
      </p>
      <div class="flex-1"></div>
      {#if meeting.app.settings.auto_paste}
        <div class="flex items-center gap-[9px] border-t border-ghost-border pt-5 text-[12.5px] text-text-muted">
          <ClipboardCheck size={15} class="shrink-0 text-accent" aria-hidden="true" />
          {$t("home.autopaste_hint")}
        </div>
      {/if}
    </div>
  {:else}
    {#if mode === "meeting" && meeting.idleSignal}
      <div class="flex items-center gap-3 rounded-default bg-warning/10 px-4 py-3 outline-1 outline-warning/30">
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

    <!-- Meeting hero: live transcript front and center. -->
    <div class="flex min-h-[300px] flex-col gap-[18px] rounded-[18px] bg-surface-1 px-6 py-[22px] outline-1 outline-ghost-border">
      <div class="flex items-center justify-between">
        <h3 class="text-sm font-semibold text-text-primary">{$t("home.live_transcript")}</h3>
        <span class="inline-flex items-center gap-1.5 text-[11.5px] text-text-muted">
          <span class={`h-1.5 w-1.5 rounded-full ${systemAudioActive ? "bg-accent" : "bg-surface-4"}`}></span>
          {systemAudioActive
            ? $t("home.system_audio_active")
            : $t("meeting_header.system_audio_unavailable")}
        </span>
      </div>
      <div
        bind:this={transcriptEl}
        onscroll={handleScroll}
        class="flex max-h-[250px] flex-1 flex-col gap-4 overflow-y-auto pr-1.5"
      >
        {#if !hasLiveContent}
          <span class="text-sm text-text-muted">{$t("home.listening")}</span>
        {:else}
          {#each liveParagraphs as paragraph, i (paragraph.id)}
            {@const label = resolveSpeakerLabel(paragraph.speaker, liveSpeakers)}
            <div class="flex flex-col gap-[3px]" style="animation: rise-in 240ms ease;">
              <div class="flex items-center gap-2">
                {#if label}
                  <span
                    class="text-[11.5px] font-semibold"
                    class:text-accent={label.kind === "me"}
                    class:text-secondary={label.kind === "them"}
                  >{label.kind === "me"
                    ? $t("transcript.me")
                    : label.kind === "them"
                      ? $t("transcript.them")
                      : label.kind === "named"
                        ? label.name
                        : $t("transcript.speaker_fallback", { values: { id: label.id } })}</span>
                {/if}
                <span class="font-mono text-[10.5px] text-text-faint">{paragraph.timestamp}</span>
              </div>
              <p class="m-0 text-[15px] leading-[1.75] text-text-secondary">
                {paragraph.text}
                {#if i === liveParagraphs.length - 1 && liveTentative}
                  <span class="opacity-50">{paragraph.text ? " " : ""}{liveTentative}</span>
                {/if}
              </p>
            </div>
          {/each}
          {#if liveParagraphs.length === 0 && liveTentative}
            <p class="m-0 text-[15px] leading-[1.75] text-text-secondary opacity-50">{liveTentative}</p>
          {/if}
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
