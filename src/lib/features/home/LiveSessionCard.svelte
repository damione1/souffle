<script lang="ts">
  import { ClipboardCheck, Square } from "@lucide/svelte";
  import { onMount } from "svelte";
  import { t } from "svelte-i18n";
  import Waveform from "../../components/Waveform.svelte";
  import Spinner from "../../components/ui/Spinner.svelte";
  import { groupIntoParagraphs } from "../../utils/paragraphs";
  import MeetingNotesSection from "../meeting/components/MeetingNotesSection.svelte";
  import type { createMeetingController } from "../meeting/controller.svelte";
  import type { createTranscriptionController } from "../transcription/controller.svelte";

  let {
    mode,
    transcription,
    meeting,
  }: {
    mode: "dictation" | "meeting";
    transcription: ReturnType<typeof createTranscriptionController>;
    meeting: ReturnType<typeof createMeetingController>;
  } = $props();

  let elapsedSeconds = $state(0);
  let transcriptEl: HTMLDivElement | undefined = $state();

  const liveText = $derived(mode === "dictation" ? transcription.transcript : "");

  // Meetings render through the same paragraph grouping as the finished
  // transcript, so diarized sessions show Me/Them live: segments are ordered
  // by time and split on speaker changes instead of one undifferentiated blob.
  const liveParagraphs = $derived(
    mode === "meeting" ? groupIntoParagraphs(meeting.liveMeetingSegments, 1.5) : [],
  );

  const hasLiveContent = $derived(mode === "dictation" ? Boolean(liveText) : liveParagraphs.length > 0);

  const elapsed = $derived(
    `${Math.floor(elapsedSeconds / 60)}:${`${elapsedSeconds % 60}`.padStart(2, "0")}`,
  );

  const stopping = $derived(
    mode === "dictation" ? transcription.isStopping : meeting.isStopping,
  );

  const systemAudioActive = $derived(Boolean(meeting.app.systemAudioStatus?.active));

  function stop() {
    if (stopping) return;
    if (mode === "dictation") {
      void transcription.toggleRecording();
    } else {
      void meeting.stopRecording();
    }
  }

  // Keep the latest words in view as they stream in.
  $effect(() => {
    void liveText;
    void liveParagraphs;
    if (transcriptEl) transcriptEl.scrollTop = transcriptEl.scrollHeight;
  });

  onMount(() => {
    const timer = setInterval(() => {
      elapsedSeconds += 1;
    }, 1000);
    return () => clearInterval(timer);
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
        class="flex max-h-[250px] flex-1 flex-col gap-4 overflow-y-auto pr-1.5"
      >
        {#if !hasLiveContent}
          <span class="text-sm text-text-muted">{$t("home.listening")}</span>
        {:else}
          {#each liveParagraphs as paragraph}
            <div class="flex flex-col gap-[3px]" style="animation: rise-in 240ms ease;">
              <div class="flex items-center gap-2">
                {#if paragraph.speaker}
                  <span
                    class="text-[11.5px] font-semibold"
                    class:text-accent={paragraph.speaker === "me"}
                    class:text-secondary={paragraph.speaker === "them"}
                  >{paragraph.speaker === "me" ? $t("transcript.me") : $t("transcript.them")}</span>
                {/if}
                <span class="font-mono text-[10.5px] text-text-faint">{paragraph.timestamp}</span>
              </div>
              <p class="m-0 text-[15px] leading-[1.75] text-text-secondary">{paragraph.text}</p>
            </div>
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
